//! Metadata-tier text extraction: turn one transcript line, or one `history.jsonl` line,
//! into zero or more `(field, flags, text)` pairs bound for a corpus shard.
//!
//! `scan::entries`/`scan::array_values` only, never `serde_json` — #20 must cold-build the
//! real corpus in under a second, and the reason `domain::scan` exists at all applies with
//! full force here. Excluded on purpose: `attachment` (the context injections — nearly as
//! numerous as `assistant` records and none of it is searched for), `system` (hook chatter),
//! every latch and event record, `summary`, and `image` blocks.

use crate::domain::cache::shard::{Field, TRUNCATED};
use crate::domain::scan::{self, Raw};
use crate::domain::session;

pub const TOOL_TEXT_CAP: usize = 4 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub field: Field,
    pub flags: u8,
    pub text: String,
    pub ts_ms: i64,
}

pub fn extract(line: &[u8]) -> Vec<Extracted> {
    let mut out = Vec::new();
    match scan::top_level_str(line, "type") {
        Some("user") => extract_user(line, &mut out),
        Some("assistant") => extract_assistant(line, &mut out),
        _ => {}
    }
    stamp(&mut out, transcript_ts_ms(line));
    out
}

pub fn extract_history(line: &[u8]) -> Vec<Extracted> {
    let mut out = Vec::new();
    if let Some(display) = scan::top_level_str(line, "display") {
        push_raw(&mut out, Field::UserPrompt, display, None);
    }
    stamp(&mut out, scan::top_level_i64(line, "timestamp").unwrap_or(0));
    out
}

fn stamp(extracted: &mut [Extracted], ts_ms: i64) {
    for item in extracted {
        item.ts_ms = ts_ms;
    }
}

fn transcript_ts_ms(line: &[u8]) -> i64 {
    scan::top_level_str(line, "timestamp")
        .and_then(|text| text.parse::<jiff::Timestamp>().ok())
        .map_or(0, jiff::Timestamp::as_millisecond)
}

fn extract_user(line: &[u8], out: &mut Vec<Extracted>) {
    let human = session::is_human_turn(&session::scan_line(line));
    let Some(message) = scan::top_level_object(line, "message") else { return };
    let Some(content) = scan::top_level_raw(message, "content") else { return };

    if let Some(text) = scan::as_str(&content) {
        if human {
            push_raw(out, Field::UserPrompt, text, None);
        }
        return;
    }
    let Some(array) = scan::as_array(&content) else { return };
    for block in scan::array_values(array) {
        extract_user_block(&block, human, out);
    }
}

fn extract_user_block(block: &Raw<'_>, human: bool, out: &mut Vec<Extracted>) {
    let Some(object) = scan::as_object(block) else { return };
    match scan::top_level_str(object, "type") {
        Some("text") if human => {
            if let Some(text) = scan::top_level_str(object, "text") {
                push_raw(out, Field::UserPrompt, text, None);
            }
        }
        Some("tool_result") => {
            let mut text = String::new();
            collect_tool_result_text(object, &mut text);
            push_owned(out, Field::ToolResult, text, Some(TOOL_TEXT_CAP));
        }
        _ => {}
    }
}

fn collect_tool_result_text(block: &[u8], out: &mut String) {
    let Some(content) = scan::top_level_raw(block, "content") else { return };
    if let Some(text) = scan::as_str(&content) {
        unescape_into(text, out);
        return;
    }
    let Some(array) = scan::as_array(&content) else { return };
    for item in scan::array_values(array) {
        let Some(object) = scan::as_object(&item) else { continue };
        if let Some(text) = scan::top_level_str(object, "text") {
            unescape_into(text, out);
            out.push('\n');
        }
    }
}

fn extract_assistant(line: &[u8], out: &mut Vec<Extracted>) {
    let Some(message) = scan::top_level_object(line, "message") else { return };
    let Some(content) = scan::top_level_raw(message, "content") else { return };

    if let Some(text) = scan::as_str(&content) {
        push_raw(out, Field::AssistantText, text, None);
        return;
    }
    let Some(array) = scan::as_array(&content) else { return };
    for block in scan::array_values(array) {
        extract_assistant_block(&block, out);
    }
}

fn extract_assistant_block(block: &Raw<'_>, out: &mut Vec<Extracted>) {
    let Some(object) = scan::as_object(block) else { return };
    match scan::top_level_str(object, "type") {
        Some("text") => {
            if let Some(text) = scan::top_level_str(object, "text") {
                push_raw(out, Field::AssistantText, text, None);
            }
        }
        Some("thinking") => {
            if let Some(text) = scan::top_level_str(object, "thinking") {
                push_raw(out, Field::Thinking, text, None);
            }
        }
        Some("tool_use") => {
            let mut text = String::new();
            if let Some(input) = scan::top_level_object(object, "input") {
                collect_object_strings(input, &mut text);
            }
            push_owned(out, Field::ToolInput, text, Some(TOOL_TEXT_CAP));
        }
        _ => {}
    }
}

fn collect_object_strings(object: &[u8], out: &mut String) {
    for (_, value) in scan::entries(object) {
        collect_value_strings(&value, out);
    }
}

fn collect_value_strings(value: &Raw<'_>, out: &mut String) {
    if let Some(text) = scan::as_str(value) {
        unescape_into(text, out);
        out.push('\n');
        return;
    }
    if let Some(object) = scan::as_object(value) {
        collect_object_strings(object, out);
        return;
    }
    if let Some(array) = scan::as_array(value) {
        for item in scan::array_values(array) {
            collect_value_strings(&item, out);
        }
    }
}

fn push_raw(out: &mut Vec<Extracted>, field: Field, raw: &str, cap_limit: Option<usize>) {
    let mut text = String::new();
    unescape_into(raw, &mut text);
    push_owned(out, field, text, cap_limit);
}

fn push_owned(out: &mut Vec<Extracted>, field: Field, text: String, cap_limit: Option<usize>) {
    if text.is_empty() {
        return;
    }
    let (text, truncated) = match cap_limit {
        Some(limit) => cap(text, limit),
        None => (text, false),
    };
    let flags = if truncated { TRUNCATED } else { 0 };
    out.push(Extracted { field, flags, text, ts_ms: 0 });
}

fn cap(text: String, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text, false);
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    (text.get(..end).unwrap_or_default().to_owned(), true)
}

fn unescape_into(raw: &str, out: &mut String) {
    let bytes = raw.as_bytes();
    let mut index = 0_usize;
    while let Some(&byte) = bytes.get(index) {
        if byte != b'\\' {
            let rest = raw.get(index..).unwrap_or_default();
            let Some(character) = rest.chars().next() else { break };
            out.push(character);
            index = index.saturating_add(character.len_utf8());
            continue;
        }

        match bytes.get(index.saturating_add(1)) {
            Some(b'n') => {
                out.push('\n');
                index = index.saturating_add(2);
            }
            Some(b't') => {
                out.push('\t');
                index = index.saturating_add(2);
            }
            Some(b'r') => {
                out.push('\r');
                index = index.saturating_add(2);
            }
            Some(b'"') => {
                out.push('"');
                index = index.saturating_add(2);
            }
            Some(b'\\') => {
                out.push('\\');
                index = index.saturating_add(2);
            }
            Some(b'/') => {
                out.push('/');
                index = index.saturating_add(2);
            }
            Some(b'u') => match read_unicode_escape(bytes, index) {
                Some((character, consumed)) => {
                    out.push(character);
                    index = index.saturating_add(consumed);
                }
                None => index = index.saturating_add(1),
            },
            _ => index = index.saturating_add(1),
        }
    }
}

fn read_unicode_escape(bytes: &[u8], backslash_at: usize) -> Option<(char, usize)> {
    let high = parse_hex4(bytes, backslash_at.checked_add(2)?)?;

    if (0xD800..=0xDBFF).contains(&high) {
        let next = backslash_at.checked_add(6)?;
        if bytes.get(next) == Some(&b'\\')
            && bytes.get(next.checked_add(1)?) == Some(&b'u')
            && let Some(low) = parse_hex4(bytes, next.checked_add(2)?)
            && (0xDC00..=0xDFFF).contains(&low)
        {
            let high_part = high.checked_sub(0xD800)?.checked_mul(0x400)?;
            let low_part = low.checked_sub(0xDC00)?;
            let code = 0x10000_u32.checked_add(high_part)?.checked_add(low_part)?;
            return char::from_u32(code).map(|character| (character, 12));
        }
        return Some((char::REPLACEMENT_CHARACTER, 6));
    }

    char::from_u32(high).map(|character| (character, 6))
}

fn parse_hex4(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    let hex = bytes.get(at..end)?;
    let text = core::str::from_utf8(hex).ok()?;
    u32::from_str_radix(text, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field_texts(extracted: &[Extracted], field: Field) -> Vec<&str> {
        extracted.iter().filter(|item| item.field == field).map(|item| item.text.as_str()).collect()
    }

    #[test]
    fn a_human_prompt_extracts_as_a_user_prompt() {
        let line =
            br#"{"type":"user","message":{"role":"user","content":"read the grid scanner back"},"origin":{"kind":"human"}}"#;
        let extracted = extract(line);
        assert_eq!(field_texts(&extracted, Field::UserPrompt), ["read the grid scanner back"]);
    }

    #[test]
    fn a_non_human_plain_string_user_record_extracts_nothing() {
        let line = br#"{"type":"user","message":{"role":"user","content":"caveat"},"isMeta":true}"#;
        assert!(extract(line).is_empty());
    }

    #[test]
    fn a_tool_result_block_extracts_as_tool_result_regardless_of_origin() {
        let line = br#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"212 src/engine/grid.rs"}]},"toolUseResult":{"stdout":"x"}}"#;
        let extracted = extract(line);
        assert_eq!(field_texts(&extracted, Field::ToolResult), ["212 src/engine/grid.rs"]);
        assert!(field_texts(&extracted, Field::UserPrompt).is_empty());
    }

    #[test]
    fn a_tool_result_content_array_of_text_blocks_is_joined() {
        let line = br#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":[{"type":"text","text":"line one"},{"type":"text","text":"line two"}]}]}}"#;
        let extracted = extract(line);
        let texts = field_texts(&extracted, Field::ToolResult);
        let joined = texts.first().copied().unwrap_or_default();
        assert!(joined.contains("line one"));
        assert!(joined.contains("line two"));
    }

    #[test]
    fn assistant_text_and_thinking_land_in_different_fields() {
        let line = br#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"pondering"},{"type":"text","text":"the answer"}]}}"#;
        let extracted = extract(line);
        assert_eq!(field_texts(&extracted, Field::Thinking), ["pondering"]);
        assert_eq!(field_texts(&extracted, Field::AssistantText), ["the answer"]);
    }

    #[test]
    fn a_tool_use_block_extracts_its_input_strings_as_tool_input() {
        let line = br#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"wc -l grid.rs","description":"count lines"}}]}}"#;
        let extracted = extract(line);
        let texts = field_texts(&extracted, Field::ToolInput);
        let joined = texts.first().copied().unwrap_or_default();
        assert!(joined.contains("wc -l grid.rs"));
        assert!(joined.contains("count lines"));
    }

    #[test]
    fn a_nested_tool_input_is_walked_recursively() {
        let line = br#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"TodoWrite","input":{"todos":[{"content":"read the scanner","status":"pending"}]}}]}}"#;
        let extracted = extract(line);
        let texts = field_texts(&extracted, Field::ToolInput);
        let joined = texts.first().copied().unwrap_or_default();
        assert!(joined.contains("read the scanner"));
        assert!(joined.contains("pending"));
    }

    #[test]
    fn an_image_block_extracts_nothing() {
        let line = br#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":""}}]}}"#;
        assert!(extract(line).is_empty());
    }

    #[test]
    fn attachment_system_and_latch_records_extract_nothing() {
        for line in [
            br#"{"type":"attachment","attachment":{"type":"date","date":"2026-01-05"}}"#.as_slice(),
            br#"{"type":"system","subtype":"compact_boundary","content":"Conversation compacted"}"#.as_slice(),
            br#"{"type":"ai-title","aiTitle":"a title"}"#.as_slice(),
            br#"{"type":"summary","summary":"legacy"}"#.as_slice(),
        ] {
            assert!(extract(line).is_empty(), "{line:?} must extract nothing");
        }
    }

    #[test]
    fn history_lines_extract_their_display_field_as_a_user_prompt() {
        let line = br#"{"display":"read the grid scanner back to me","sessionId":"s1"}"#;
        let extracted = extract_history(line);
        assert_eq!(field_texts(&extracted, Field::UserPrompt), ["read the grid scanner back to me"]);
    }

    #[test]
    fn a_transcript_record_carries_its_iso8601_timestamp_as_milliseconds() {
        let line = br#"{"type":"user","message":{"role":"user","content":"hi"},"origin":{"kind":"human"},"timestamp":"2026-01-05T09:00:00Z"}"#;
        let extracted = extract(line);
        let expected = "2026-01-05T09:00:00Z".parse::<jiff::Timestamp>().expect("a valid timestamp").as_millisecond();
        assert_eq!(extracted.first().map(|item| item.ts_ms), Some(expected));
    }

    #[test]
    fn a_history_line_carries_its_raw_millisecond_timestamp() {
        let line = br#"{"display":"hi","timestamp":1767610800000,"sessionId":"s1"}"#;
        let extracted = extract_history(line);
        assert_eq!(extracted.first().map(|item| item.ts_ms), Some(1_767_610_800_000));
    }

    #[test]
    fn a_record_with_no_timestamp_stamps_zero() {
        let line = br#"{"type":"user","message":{"role":"user","content":"hi"},"origin":{"kind":"human"}}"#;
        let extracted = extract(line);
        assert_eq!(extracted.first().map(|item| item.ts_ms), Some(0));
    }

    #[test]
    fn tool_output_over_the_cap_is_truncated_at_a_char_boundary_and_flagged() {
        let big: String = std::iter::repeat_n('é', 3000).collect();
        let escaped = big.replace('é', "\\u00e9");
        let line = format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","content":"{escaped}"}}]}}}}"#
        );
        let extracted = extract(line.as_bytes());
        let record = extracted.iter().find(|item| item.field == Field::ToolResult).expect("a tool result");
        assert!(record.text.len() <= TOOL_TEXT_CAP);
        assert_eq!(record.flags, TRUNCATED);
        assert!(core::str::from_utf8(record.text.as_bytes()).is_ok(), "truncation must land on a char boundary");
    }

    #[test]
    fn a_short_tool_result_is_not_flagged_truncated() {
        let line = br#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"short"}]}}"#;
        let extracted = extract(line);
        let record = extracted.iter().find(|item| item.field == Field::ToolResult).expect("a tool result");
        assert_eq!(record.flags, 0);
    }

    #[test]
    fn escape_sequences_round_trip_through_unescaping() {
        let line = br#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"line one\nline two\ta \"quoted\" word and a \\ backslash"}]}}"#;
        let extracted = extract(line);
        let text = field_texts(&extracted, Field::AssistantText).into_iter().next().expect("assistant text");
        assert_eq!(text, "line one\nline two\ta \"quoted\" word and a \\ backslash");
    }

    #[test]
    fn a_unicode_escape_decodes_to_its_character() {
        let line = "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"caf\\u00e9\"}]}}";
        let extracted = extract(line.as_bytes());
        let text = field_texts(&extracted, Field::AssistantText).into_iter().next().expect("assistant text");
        assert_eq!(text, "caf\u{e9}");
    }

    #[test]
    fn a_surrogate_pair_decodes_to_one_character_outside_the_bmp() {
        let line = "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"\\ud83d\\ude00\"}]}}";
        let extracted = extract(line.as_bytes());
        let text = field_texts(&extracted, Field::AssistantText).into_iter().next().expect("assistant text");
        assert_eq!(text, "\u{1F600}");
    }

    #[test]
    fn a_lone_surrogate_falls_back_to_the_replacement_character_rather_than_panicking() {
        let line = br#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"a \ud800 b"}]}}"#;
        let extracted = extract(line);
        let text = field_texts(&extracted, Field::AssistantText).into_iter().next().expect("assistant text");
        assert!(text.contains('\u{FFFD}'));
    }
}
