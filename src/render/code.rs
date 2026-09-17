//! Syntax highlighting for fenced code blocks, and the process-global cache in front of it.

use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use ratatui::style::{Color, Modifier, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SyntectStyle, Theme as SyntectTheme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::render::line::{self, StyledSpan};

const THEME: &str = "base16-ocean.dark";
const PLAIN: &str = "Plain Text";
const CAPACITY: usize = 256;

pub type CodeLines = Arc<Vec<Vec<StyledSpan>>>;

pub fn highlight(lang: Option<&str>, text: &str) -> CodeLines {
    let syntaxes = syntax_set();
    let syntax = syntax_for(syntaxes, lang, text);

    let key =
        Key { syntax: syntax.map_or("", |syntax| syntax.name.as_str()).to_owned(), theme: THEME.to_owned(), text: hash(text) };
    if let Some(hit) = cached(&key) {
        return hit;
    }

    let lines = Arc::new(match (syntax, theme()) {
        (Some(syntax), Some(theme)) => paint(syntaxes, syntax, theme, text),
        _ => unpainted(text),
    });
    store(key, Arc::clone(&lines));
    lines
}

fn syntax_for<'a>(syntaxes: &'a SyntaxSet, lang: Option<&str>, text: &str) -> Option<&'a SyntaxReference> {
    lang.map(str::trim)
        .filter(|lang| !lang.is_empty())
        .and_then(|lang| syntaxes.find_syntax_by_token(lang))
        .or_else(|| text.lines().next().and_then(|first| syntaxes.find_syntax_by_first_line(first)))
        .filter(|syntax| syntax.name != PLAIN)
}

fn paint(syntaxes: &SyntaxSet, syntax: &SyntaxReference, theme: &SyntectTheme, text: &str) -> Vec<Vec<StyledSpan>> {
    let mut highlighter = HighlightLines::new(syntax, theme);
    let body = body(text);
    LinesWithEndings::from(&body)
        .map(|line| {
            highlighter.highlight_line(line, syntaxes).map_or_else(
                |_| plain(line).into_iter().collect(),
                |regions| line::merge(regions.iter().filter_map(|(style, piece)| span(*style, piece))),
            )
        })
        .collect()
}

fn unpainted(text: &str) -> Vec<Vec<StyledSpan>> {
    LinesWithEndings::from(&body(text)).map(|line| plain(line).into_iter().collect()).collect()
}

fn body(text: &str) -> String {
    let trimmed = text.trim_end_matches('\n');
    if trimmed.is_empty() { String::new() } else { format!("{trimmed}\n") }
}

fn plain(line: &str) -> Option<StyledSpan> {
    let line = line.trim_end_matches(['\n', '\r']);
    (!line.is_empty()).then(|| StyledSpan::new(line, Style::default()))
}

fn span(style: SyntectStyle, piece: &str) -> Option<StyledSpan> {
    let piece = piece.trim_end_matches(['\n', '\r']);
    (!piece.is_empty()).then(|| StyledSpan::new(piece, convert(style)))
}

fn convert(style: SyntectStyle) -> Style {
    let foreground = style.foreground;
    let mut converted = Style::default().fg(Color::Rgb(foreground.r, foreground.g, foreground.b));

    for (font_style, modifier) in
        [(FontStyle::BOLD, Modifier::BOLD), (FontStyle::ITALIC, Modifier::ITALIC), (FontStyle::UNDERLINE, Modifier::UNDERLINED)]
    {
        if style.font_style.contains(font_style) {
            converted = converted.add_modifier(modifier);
        }
    }
    converted
}

fn syntax_set() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(|| {
        syntect::dumps::from_uncompressed_data(include_bytes!(concat!(env!("OUT_DIR"), "/syntaxes.pack")))
            .unwrap_or_else(|_| SyntaxSet::new())
    })
}

fn theme() -> Option<&'static SyntectTheme> {
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    THEMES.get_or_init(ThemeSet::load_defaults).themes.get(THEME)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    syntax: String,
    theme: String,
    text: u64,
}

#[derive(Default)]
struct Cache {
    blocks: HashMap<Key, CodeLines>,
    order: VecDeque<Key>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn cached(key: &Key) -> Option<CodeLines> {
    cache().lock().unwrap_or_else(PoisonError::into_inner).blocks.get(key).map(Arc::clone)
}

fn store(key: Key, lines: CodeLines) {
    let mut cache = cache().lock().unwrap_or_else(PoisonError::into_inner);
    if cache.blocks.insert(key.clone(), lines).is_none() {
        cache.order.push_back(key);
    }
    while cache.order.len() > CAPACITY {
        let Some(oldest) = cache.order.pop_front() else { break };
        cache.blocks.remove(&oldest);
    }
    drop(cache);
}

fn hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(lines: &[Vec<StyledSpan>]) -> Vec<Style> {
        lines.iter().flatten().map(|span| span.style).collect()
    }

    #[test]
    fn the_bundled_pack_carries_more_than_syntects_own_defaults() {
        let syntaxes = syntax_set();
        for token in ["rust", "swift", "kotlin", "toml", "ts", "mermaid"] {
            assert!(syntaxes.find_syntax_by_token(token).is_some(), "no syntax for {token:?}");
        }
    }

    #[test]
    fn a_known_language_comes_back_in_more_than_one_colour() {
        let lines = highlight(Some("rust"), "fn main() {\n    let x = \"hi\";\n}\n");
        assert_eq!(lines.len(), 3);
        let styles = styles(&lines);
        assert!(styles.iter().collect::<std::collections::HashSet<_>>().len() > 1, "{styles:?}");
    }

    #[test]
    fn a_language_nobody_bundles_falls_back_to_one_unstyled_span_a_line() {
        let lines = highlight(Some("nothing-of-the-sort"), "alpha\nbeta\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(styles(&lines), vec![Style::default(), Style::default()]);
    }

    #[test]
    fn a_fence_with_no_language_at_all_is_still_laid_out_line_by_line() {
        let lines = highlight(None, "alpha\nbeta\ngamma\n");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn the_same_block_twice_is_the_same_allocation_rather_than_a_second_highlight() {
        let first = highlight(Some("rust"), "fn cached() {}\n");
        let second = highlight(Some("rust"), "fn cached() {}\n");
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn the_language_is_part_of_the_key_so_two_readings_of_one_text_do_not_collide() {
        let source = "let x = 1\n";
        assert!(!Arc::ptr_eq(&highlight(Some("rust"), source), &highlight(Some("swift"), source)));
    }

    #[test]
    fn a_trailing_newline_does_not_add_an_empty_line_at_the_end() {
        assert_eq!(highlight(Some("rust"), "fn main() {}").len(), 1);
        assert_eq!(highlight(Some("rust"), "fn main() {}\n").len(), 1);
        assert_eq!(highlight(Some("rust"), "fn main() {}\n\n").len(), 1);
    }

    #[test]
    fn an_empty_block_is_no_lines_rather_than_one_blank_one() {
        assert!(highlight(Some("rust"), "").is_empty());
        assert!(highlight(Some("rust"), "\n\n").is_empty());
    }

    #[test]
    fn the_cache_evicts_rather_than_growing_without_a_bound() {
        for index in 0..CAPACITY.saturating_add(16) {
            highlight(Some("rust"), &format!("fn evicted{index}() {{}}\n"));
        }
        assert!(cache().lock().unwrap_or_else(PoisonError::into_inner).order.len() <= CAPACITY);
    }
}
