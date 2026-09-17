//! The contract every renderer speaks: styled spans, a laid-out line, and the display-width
//! arithmetic that turns prose into lines of a given column count.

use ratatui::style::Style;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const TAB_WIDTH: usize = 4;
const ELLIPSIS: &str = "…";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledSpan {
    pub text: String,
    pub style: Style,
}

impl StyledSpan {
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self { text: text.into(), style }
    }

    pub fn width(&self) -> usize {
        self.text.width()
    }
}

pub fn merge(spans: impl IntoIterator<Item = StyledSpan>) -> Vec<StyledSpan> {
    spans.into_iter().filter(|span| !span.text.is_empty()).fold(Vec::new(), |mut merged: Vec<StyledSpan>, span| {
        match merged.last_mut() {
            Some(last) if last.style == span.style => last.text.push_str(&span.text),
            _ => merged.push(span),
        }
        merged
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RenderedLine {
    pub spans: Vec<StyledSpan>,
    pub inset: usize,
}

impl RenderedLine {
    pub fn blank() -> Self {
        Self::default()
    }

    pub fn is_blank(&self) -> bool {
        self.spans.iter().all(|span| span.text.is_empty())
    }

    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    pub fn width(&self) -> usize {
        self.spans.iter().map(StyledSpan::width).fold(0, usize::saturating_add)
    }

    pub fn push(&mut self, span: StyledSpan) {
        self.spans.push(span);
    }

    pub fn prefix(&mut self, span: StyledSpan) {
        self.inset = self.inset.saturating_add(span.width());
        self.spans.insert(0, span);
    }
}

pub fn split_first_char(text: &str) -> (&str, &str) {
    text.split_at_checked(text.chars().next().map_or(0, char::len_utf8)).unwrap_or((text, ""))
}

pub fn split_at_width(text: &str, width: usize) -> (&str, &str) {
    let mut used = 0usize;
    for (offset, character) in text.char_indices() {
        let next = used.saturating_add(UnicodeWidthChar::width(character).unwrap_or(0));
        if next > width {
            return text.split_at_checked(offset).unwrap_or((text, ""));
        }
        used = next;
    }
    (text, "")
}

pub fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let (head, _) = split_at_width(text, width.saturating_sub(ELLIPSIS.width()));
    format!("{head}{ELLIPSIS}")
}

pub fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\u{1b}' => skip_escape(&mut characters),
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                out.push('\n');
            }
            '\n' => out.push('\n'),
            '\t' => out.push_str(&" ".repeat(TAB_WIDTH)),
            _ if UnicodeWidthChar::width(character).is_none() => {}
            _ => out.push(character),
        }
    }
    out
}

fn skip_escape(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    if characters.peek() != Some(&'[') {
        characters.next();
        return;
    }
    characters.next();
    while characters.next_if(|&c| !matches!(c, '@'..='~')).is_some() {}
    characters.next();
}

pub fn wrap(text: &str, style: Style, width: usize) -> Vec<RenderedLine> {
    let normalised = normalise(text);
    let mut lines = Vec::new();
    for segment in normalised.split('\n') {
        let wrapped = Wrapper::new(width).run(segment, style);
        if wrapped.is_empty() {
            lines.push(RenderedLine::blank());
        } else {
            lines.extend(wrapped);
        }
    }
    lines
}

struct Wrapper {
    width: usize,
    lines: Vec<RenderedLine>,
    current: RenderedLine,
    used: usize,
    pending_space: bool,
}

enum Token<'a> {
    Word(&'a str),
    Space(&'a str),
}

impl Wrapper {
    fn new(width: usize) -> Self {
        Self {
            width: if width == 0 { 1 } else { width },
            lines: Vec::new(),
            current: RenderedLine::blank(),
            used: 0,
            pending_space: false,
        }
    }

    fn run(mut self, segment: &str, style: Style) -> Vec<RenderedLine> {
        for (index, token) in tokenize(segment).into_iter().enumerate() {
            match token {
                Token::Space(indent) if index == 0 => self.word(indent, style),
                Token::Space(_) if !self.current.spans.is_empty() => self.pending_space = true,
                Token::Space(_) => {}
                Token::Word(word) => self.word(word, style),
            }
        }
        self.finish()
    }

    fn word(&mut self, word: &str, style: Style) {
        let space = usize::from(self.pending_space);
        if !self.current.spans.is_empty() && self.used.saturating_add(space).saturating_add(word.width()) > self.width {
            self.newline();
        }
        if self.pending_space {
            self.pending_space = false;
            self.emit(" ", style);
        }

        let mut rest = word;
        loop {
            let room = self.width.saturating_sub(self.used);
            if rest.width() <= room {
                break;
            }
            let (head, tail) = split_at_width(rest, room);
            if head.is_empty() {
                if !self.current.spans.is_empty() {
                    self.newline();
                    continue;
                }
                let (head, tail) = split_first_char(rest);
                self.emit(head, style);
                self.newline();
                rest = tail;
                continue;
            }
            self.emit(head, style);
            self.newline();
            rest = tail;
        }
        if !rest.is_empty() {
            self.emit(rest, style);
        }
    }

    fn emit(&mut self, text: &str, style: Style) {
        self.pending_space = false;
        match self.current.spans.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.current.push(StyledSpan::new(text, style)),
        }
        self.used = self.used.saturating_add(text.width());
    }

    fn newline(&mut self) {
        let finished = std::mem::replace(&mut self.current, RenderedLine::blank());
        self.lines.push(finished);
        self.used = 0;
        self.pending_space = false;
    }

    fn finish(mut self) -> Vec<RenderedLine> {
        if !self.current.spans.is_empty() {
            self.lines.push(self.current);
        }
        self.lines
    }
}

fn tokenize(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let whitespace = rest.starts_with(char::is_whitespace);
        let end = rest.find(|c: char| c.is_whitespace() != whitespace).unwrap_or(rest.len());
        let Some((head, tail)) = rest.split_at_checked(end) else { break };
        tokens.push(if whitespace { Token::Space(head) } else { Token::Word(head) });
        rest = tail;
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Style {
        Style::default()
    }

    fn texts(lines: &[RenderedLine]) -> Vec<String> {
        lines.iter().map(RenderedLine::text).collect()
    }

    #[test]
    fn width_counts_display_columns_rather_than_bytes() {
        assert_eq!(StyledSpan::new("日本語", plain()).width(), 6);
        assert_eq!(StyledSpan::new("hello", plain()).width(), 5);
    }

    #[test]
    fn splitting_respects_character_boundaries_and_display_width() {
        assert_eq!(split_at_width("日本語", 3), ("日", "本語"));
        assert_eq!(split_at_width("日本語", 4), ("日本", "語"));
        assert_eq!(split_at_width("abc", 10), ("abc", ""));
    }

    #[test]
    fn truncation_marks_the_cut_at_the_right_display_width() {
        assert_eq!(truncate("日本語", 4), "日…");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello there", 7), "hello …");
    }

    #[test]
    fn adjacent_spans_of_one_style_merge_and_empty_spans_vanish() {
        let merged = merge([
            StyledSpan::new("hel", plain()),
            StyledSpan::new("", plain()),
            StyledSpan::new("lo", plain()),
            StyledSpan::new("!", plain().add_modifier(ratatui::style::Modifier::BOLD)),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged.first().map(|span| span.text.as_str()), Some("hello"));
    }

    #[test]
    fn a_prefix_goes_to_column_zero_and_records_the_inset() {
        let mut line = RenderedLine::blank();
        line.push(StyledSpan::new("text", plain()));
        line.prefix(StyledSpan::new("▎ ", plain()));
        assert_eq!(line.text(), "▎ text");
        assert_eq!(line.inset, 2);
    }

    #[test]
    fn wrapping_never_exceeds_the_width() {
        let prose = "the grid scanner reads the deflector array back to me one plate at a time";
        for width in 1..=40 {
            for line in wrap(prose, plain(), width) {
                assert!(line.width() <= width, "width {width} overflowed with {:?}", line.text());
            }
        }
    }

    #[test]
    fn wrapping_breaks_between_words_and_never_ends_a_line_on_a_space() {
        let lines = wrap("alpha beta gamma delta", plain(), 12);
        assert_eq!(texts(&lines), vec!["alpha beta", "gamma delta"]);
    }

    #[test]
    fn newlines_survive_the_wrap_rather_than_reflowing_into_one_blob() {
        let lines = wrap("1. first\n2. second\n\n3. third", plain(), 40);
        assert_eq!(texts(&lines), vec!["1. first", "2. second", "", "3. third"]);
    }

    #[test]
    fn leading_indentation_is_kept_so_a_nested_list_still_reads_as_one() {
        let lines = wrap("outer\n    inner", plain(), 40);
        assert_eq!(texts(&lines), vec!["outer", "    inner"]);
    }

    #[test]
    fn a_wide_glyph_in_a_one_column_box_makes_progress_instead_of_looping() {
        let lines = wrap("日本語", plain(), 1);
        assert_eq!(texts(&lines), vec!["日", "本", "語"]);
    }

    #[test]
    fn a_word_longer_than_the_width_is_broken_at_a_display_column() {
        let lines = wrap("日本語テスト", plain(), 4);
        assert_eq!(texts(&lines), vec!["日本", "語テ", "スト"]);
    }

    #[test]
    fn an_emoji_counts_as_the_two_columns_a_terminal_gives_it() {
        assert_eq!(StyledSpan::new("🖖", plain()).width(), 2);
        for line in wrap("🖖 live long 🖖 and prosper", plain(), 9) {
            assert!(line.width() <= 9, "{:?}", line.text());
        }
    }

    #[test]
    fn a_tab_becomes_spaces_and_a_control_character_is_dropped_before_anything_measures_it() {
        assert_eq!(normalise("a\tb"), "a    b");
        assert_eq!(normalise("a\u{7}b"), "ab");
        assert_eq!(normalise("a\r\nb"), "a\nb");
        assert_eq!(normalise("a\rb"), "a\nb");
    }

    #[test]
    fn an_ansi_escape_sequence_never_reaches_the_width_arithmetic() {
        assert_eq!(normalise("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(wrap("\u{1b}[31mred\u{1b}[0m", plain(), 10).first().map(RenderedLine::width), Some(3));
    }

    #[test]
    fn an_empty_string_is_one_blank_line_rather_than_none() {
        assert_eq!(wrap("", plain(), 10), vec![RenderedLine::blank()]);
        assert!(RenderedLine::blank().is_blank());
    }

    #[test]
    fn a_zero_width_box_does_not_loop_forever() {
        let lines = wrap("alpha beta", plain(), 0);
        assert!(!lines.is_empty());
        assert!(lines.iter().all(|line| line.width() <= 1));
    }
}
