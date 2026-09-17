//! The metadata tier: reading one top-level key out of a JSONL record without `serde_json`.

const WHITESPACE: &[u8] = b" \t\n\r";

enum Raw<'a> {
    Str(&'a [u8]),
    Other(&'a [u8]),
}

pub fn top_level_str<'a>(line: &'a [u8], key: &str) -> Option<&'a str> {
    match top_level_raw(line, key)? {
        Raw::Str(span) => core::str::from_utf8(span).ok(),
        Raw::Other(_) => None,
    }
}

pub fn top_level_is_null(line: &[u8], key: &str) -> bool {
    matches!(top_level_raw(line, key), Some(Raw::Other(b"null")))
}

fn top_level_raw<'a>(line: &'a [u8], key: &str) -> Option<Raw<'a>> {
    let mut rest = line;
    skip_whitespace(&mut rest);

    let (&opening, after) = rest.split_first()?;
    if opening != b'{' {
        return None;
    }
    rest = after;

    loop {
        skip_whitespace(&mut rest);
        let (&byte, after) = rest.split_first()?;
        match byte {
            b',' => {
                rest = after;
                continue;
            }
            b'"' => rest = after,
            _ => return None,
        }

        let name = read_string(&mut rest)?;

        skip_whitespace(&mut rest);
        let (&colon, after) = rest.split_first()?;
        if colon != b':' {
            return None;
        }
        rest = after;
        skip_whitespace(&mut rest);

        let value = read_value(&mut rest)?;
        if name == key.as_bytes() {
            return Some(value);
        }
    }
}

fn read_value<'a>(rest: &mut &'a [u8]) -> Option<Raw<'a>> {
    match *rest.first()? {
        b'"' => {
            let (_, after) = rest.split_first()?;
            *rest = after;
            read_string(rest).map(Raw::Str)
        }
        b'{' | b'[' => skip_nested(rest).map(Raw::Other),
        _ => Some(Raw::Other(read_scalar(rest))),
    }
}

fn read_string<'a>(rest: &mut &'a [u8]) -> Option<&'a [u8]> {
    let mut escaped = false;
    let mut closing = None;
    for (index, &byte) in rest.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'"' => {
                closing = Some(index);
                break;
            }
            _ => {}
        }
    }

    let (content, tail) = rest.split_at_checked(closing?)?;
    let (_, after) = tail.split_first()?;
    *rest = after;
    Some(content)
}

fn skip_nested<'a>(rest: &mut &'a [u8]) -> Option<&'a [u8]> {
    let start = *rest;
    let mut depth: usize = 0;

    loop {
        let (&byte, after) = rest.split_first()?;
        match byte {
            b'{' | b'[' => {
                depth = depth.checked_add(1)?;
                *rest = after;
            }
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                *rest = after;
                if depth == 0 {
                    break;
                }
            }
            b'"' => {
                *rest = after;
                read_string(rest)?;
            }
            _ => *rest = after,
        }
    }

    let consumed = start.len().checked_sub(rest.len())?;
    start.split_at_checked(consumed).map(|(value, _)| value)
}

fn read_scalar<'a>(rest: &mut &'a [u8]) -> &'a [u8] {
    let start = *rest;
    let end = start.iter().position(|&byte| is_terminator(byte)).unwrap_or(start.len());
    if let Some((value, tail)) = start.split_at_checked(end) {
        *rest = tail;
        value
    } else {
        *rest = &[];
        start
    }
}

fn is_terminator(byte: u8) -> bool {
    matches!(byte, b',' | b'}' | b']') || WHITESPACE.contains(&byte)
}

fn skip_whitespace(rest: &mut &[u8]) {
    while let Some((byte, after)) = rest.split_first() {
        if !WHITESPACE.contains(byte) {
            break;
        }
        *rest = after;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_top_level_string_is_borrowed_from_the_line() {
        let line = br#"{"parentUuid":null,"type":"assistant","uuid":"abc"}"#;
        assert_eq!(top_level_str(line, "type"), Some("assistant"));
        assert_eq!(top_level_str(line, "uuid"), Some("abc"));
    }

    #[test]
    fn a_nested_type_key_inside_message_is_not_returned() {
        let line =
            br#"{"parentUuid":null,"message":{"role":"assistant","content":[{"type":"text","text":"hi"}]},"type":"assistant"}"#;
        assert_eq!(top_level_str(line, "type"), Some("assistant"));
    }

    #[test]
    fn an_object_valued_key_is_not_a_string() {
        let line = br#"{"message":{"type":"message"},"type":"assistant"}"#;
        assert_eq!(top_level_str(line, "message"), None);
    }

    #[test]
    fn an_array_of_objects_is_skipped_whole() {
        let line = br#"{"content":[{"type":"text"},{"type":"image","source":{"type":"base64"}}],"type":"user"}"#;
        assert_eq!(top_level_str(line, "type"), Some("user"));
    }

    #[test]
    fn a_nested_brace_inside_a_string_does_not_unbalance_the_walk() {
        let line = br#"{"message":{"text":"a } and a ] and a \" quote"},"type":"assistant"}"#;
        assert_eq!(top_level_str(line, "type"), Some("assistant"));
    }

    #[test]
    fn an_escaped_quote_inside_a_key_does_not_end_the_key() {
        let line = br#"{"ty\"pe":"decoy","type":"real"}"#;
        assert_eq!(top_level_str(line, "type"), Some("real"));
    }

    #[test]
    fn an_escaped_backslash_before_a_quote_ends_the_string() {
        let line = br#"{"cwd":"C:\\","type":"user"}"#;
        assert_eq!(top_level_str(line, "cwd"), Some(r"C:\\"));
        assert_eq!(top_level_str(line, "type"), Some("user"));
    }

    #[test]
    fn a_missing_key_is_none() {
        let line = br#"{"type":"user"}"#;
        assert_eq!(top_level_str(line, "sessionId"), None);
        assert!(!top_level_is_null(line, "sessionId"));
    }

    #[test]
    fn a_null_value_is_null_and_not_a_string() {
        let line = br#"{"parentUuid":null,"type":"user"}"#;
        assert!(top_level_is_null(line, "parentUuid"));
        assert_eq!(top_level_str(line, "parentUuid"), None);
    }

    #[test]
    fn a_string_reading_null_is_not_null() {
        let line = br#"{"parentUuid":"null"}"#;
        assert!(!top_level_is_null(line, "parentUuid"));
        assert_eq!(top_level_str(line, "parentUuid"), Some("null"));
    }

    #[test]
    fn a_number_and_a_boolean_are_neither_string_nor_null() {
        let line = br#"{"apiBlockIndex":2,"isSidechain":false,"type":"assistant"}"#;
        assert_eq!(top_level_str(line, "apiBlockIndex"), None);
        assert!(!top_level_is_null(line, "isSidechain"));
        assert_eq!(top_level_str(line, "type"), Some("assistant"));
    }

    #[test]
    fn an_empty_line_yields_nothing() {
        assert_eq!(top_level_str(b"", "type"), None);
        assert!(!top_level_is_null(b"", "type"));
    }

    #[test]
    fn a_line_that_is_not_an_object_yields_nothing() {
        assert_eq!(top_level_str(b"[1,2,3]", "type"), None);
        assert_eq!(top_level_str(b"not json at all", "type"), None);
    }

    #[test]
    fn a_truncated_line_yields_the_keys_that_survived_the_cut() {
        let line = br#"{"type":"user","message":{"role":"user","cont"#;
        assert_eq!(top_level_str(line, "type"), Some("user"));
        assert_eq!(top_level_str(line, "uuid"), None);
    }

    #[test]
    fn a_key_after_a_truncated_value_is_not_invented() {
        let line = br#"{"message":{"role":"user","cont"#;
        assert_eq!(top_level_str(line, "type"), None);
    }

    #[test]
    fn a_non_utf8_byte_sequence_poisons_only_its_own_value() {
        let mut line = Vec::new();
        line.extend_from_slice(br#"{"text":""#);
        line.extend_from_slice(&[0xff, 0xfe]);
        line.extend_from_slice(br#"","type":"user"}"#);
        assert_eq!(top_level_str(&line, "text"), None);
        assert_eq!(top_level_str(&line, "type"), Some("user"));
    }

    #[test]
    fn whitespace_around_the_structure_is_tolerated() {
        let line = b"  { \"type\" : \"user\" , \"uuid\" : \"a\" }  ";
        assert_eq!(top_level_str(line, "type"), Some("user"));
        assert_eq!(top_level_str(line, "uuid"), Some("a"));
    }

    #[test]
    fn an_empty_object_and_an_empty_array_are_skipped() {
        let line = br#"{"a":{},"b":[],"type":"user"}"#;
        assert_eq!(top_level_str(line, "type"), Some("user"));
    }

    #[test]
    fn an_empty_string_value_is_a_string() {
        let line = br#"{"gitBranch":"","type":"user"}"#;
        assert_eq!(top_level_str(line, "gitBranch"), Some(""));
    }

    #[test]
    fn a_duplicate_key_reads_as_the_first_occurrence() {
        let line = br#"{"type":"user","type":"assistant"}"#;
        assert_eq!(top_level_str(line, "type"), Some("user"));
    }
}
