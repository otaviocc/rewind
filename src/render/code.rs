//! Syntax highlighting for fenced code blocks, and the process-global cache in front of it.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use ratatui::style::{Color, Modifier, Style};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SyntectStyle, Theme as SyntectTheme, ThemeSet};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use crate::render::line::{self, StyledSpan};
use crate::theme::{Element, Theme as RewindTheme};

const FALLBACK: &str = "base16-ocean.dark";
const BUILT_IN: [(&str, &[u8]); 1] = [("default-plus", include_bytes!("../../syntax-themes/default-plus.tmTheme"))];
const PLAIN: &str = "Plain Text";
const CAPACITY: usize = 256;

pub type CodeLines = Arc<Vec<Vec<StyledSpan>>>;

pub fn highlight(lang: Option<&str>, text: &str, theme: &RewindTheme) -> CodeLines {
    let syntaxes = syntax_set();
    let syntax = syntax_for(syntaxes, lang, text);
    let fallback = theme.style(Element::CodeBlock);

    let (name, syntect_theme) = syntect_theme(theme.syntax_theme.as_deref());

    let key =
        Key { syntax: syntax.map_or("", |syntax| syntax.name.as_str()).to_owned(), theme: name.to_owned(), text: hash(text) };
    if let Some(hit) = cached(&key) {
        return hit;
    }

    let lines = Arc::new(match (syntax, syntect_theme) {
        (Some(syntax), Some(syntect_theme)) => paint(syntaxes, syntax, syntect_theme, text),
        _ => unpainted(text, fallback),
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

fn paint(syntaxes: &SyntaxSet, syntax: &SyntaxReference, syntect_theme: &SyntectTheme, text: &str) -> Vec<Vec<StyledSpan>> {
    let mut highlighter = HighlightLines::new(syntax, syntect_theme);
    let body = body(text);
    LinesWithEndings::from(&body)
        .map(|line| {
            highlighter.highlight_line(line, syntaxes).map_or_else(
                |_| plain(line, Style::default()).into_iter().collect(),
                |regions| line::merge(regions.iter().filter_map(|(style, piece)| span(*style, piece))),
            )
        })
        .collect()
}

fn unpainted(text: &str, fallback: Style) -> Vec<Vec<StyledSpan>> {
    LinesWithEndings::from(&body(text)).map(|line| plain(line, fallback).into_iter().collect()).collect()
}

fn body(text: &str) -> String {
    let trimmed = text.trim_end_matches('\n');
    if trimmed.is_empty() { String::new() } else { format!("{trimmed}\n") }
}

fn plain(line: &str, style: Style) -> Option<StyledSpan> {
    let line = line.trim_end_matches(['\n', '\r']);
    (!line.is_empty()).then(|| StyledSpan::new(line, style))
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

fn theme_set() -> &'static ThemeSet {
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    THEMES.get_or_init(|| {
        let mut themes = ThemeSet::load_defaults();
        for (name, bytes) in BUILT_IN {
            let Ok(theme) = ThemeSet::load_from_reader(&mut Cursor::new(bytes)) else { continue };
            themes.themes.insert(name.to_owned(), theme);
        }
        themes
    })
}

fn syntect_theme(name: Option<&str>) -> (&'static str, Option<&'static SyntectTheme>) {
    let themes = theme_set();

    if let Some(name) = name {
        if let Some((name, theme)) = themes.themes.get_key_value(name) {
            return (name, Some(theme));
        }
        warn(name);
    }

    (FALLBACK, themes.themes.get(FALLBACK))
}

fn warn(name: &str) {
    static SAID: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let said = SAID.get_or_init(|| Mutex::new(BTreeSet::new()));
    let mut said = said.lock().unwrap_or_else(PoisonError::into_inner);
    if said.insert(name.to_owned()) {
        eprintln!("rewind: no syntax theme named {name:?}; using {FALLBACK}");
    }
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

    fn theme() -> RewindTheme {
        RewindTheme::default()
    }

    fn themed(syntax_theme: &str) -> RewindTheme {
        let mut theme = RewindTheme::default();
        theme.syntax_theme = Some(syntax_theme.to_owned());
        theme
    }

    fn styles(lines: &[Vec<StyledSpan>]) -> Vec<Style> {
        lines.iter().flatten().map(|span| span.style).collect()
    }

    #[test]
    fn a_theme_that_names_a_syntax_theme_paints_a_different_set_of_colours() {
        const CODE: &str = "fn main() {\n    let x = \"hi\";\n}\n";
        let ocean = styles(&highlight(Some("rust"), CODE, &themed("base16-ocean.dark")));
        let mocha = styles(&highlight(Some("rust"), CODE, &themed("base16-mocha.dark")));
        assert_ne!(ocean, mocha, "two syntax themes painted the same block identically");
    }

    #[test]
    fn a_syntax_theme_nobody_bundles_falls_back_rather_than_dropping_the_highlighting() {
        const CODE: &str = "fn fallen_back() {}\n";
        let unknown = styles(&highlight(Some("rust"), CODE, &themed("no-such-syntax-theme")));
        let fallback = styles(&highlight(Some("rust"), CODE, &themed(FALLBACK)));
        assert_eq!(unknown, fallback, "an unknown syntax theme did not land on the fallback");
        assert!(unknown.iter().collect::<std::collections::HashSet<_>>().len() > 1, "the highlighting was dropped");
    }

    #[test]
    fn the_bundled_syntax_theme_is_found_rather_than_falling_back() {
        let (name, theme) = syntect_theme(Some("default-plus"));
        assert_eq!(name, "default-plus");
        assert!(theme.is_some(), "the bundled .tmTheme did not load");
    }

    #[test]
    fn default_plus_paints_comments_green_and_strings_red() {
        let lines = highlight(Some("rust"), "// note\nlet s = \"hi\";\n", &themed("default-plus"));
        let colours: Vec<Option<Color>> = lines.iter().flatten().map(|span| span.style.fg).collect();
        assert!(colours.contains(&Some(Color::Rgb(0x2e, 0xa8, 0x5b))), "the comment is not Default+ green: {colours:?}");
        assert!(colours.contains(&Some(Color::Rgb(0xfc, 0x46, 0x51))), "the string is not Default+ red: {colours:?}");
    }

    #[test]
    fn saying_nothing_about_a_syntax_theme_is_the_fallback() {
        const CODE: &str = "fn unsaid() {}\n";
        assert_eq!(styles(&highlight(Some("rust"), CODE, &theme())), styles(&highlight(Some("rust"), CODE, &themed(FALLBACK))));
    }

    #[test]
    fn the_cache_does_not_hand_one_syntax_theme_another_ones_colours() {
        const CODE: &str = "fn cache_keyed_by_theme() {}\n";
        let first = styles(&highlight(Some("rust"), CODE, &themed("base16-ocean.dark")));
        let second = styles(&highlight(Some("rust"), CODE, &themed("InspiredGitHub")));
        assert_ne!(first, second, "the second theme was served the first one's cached block");
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
        let lines = highlight(Some("rust"), "fn main() {\n    let x = \"hi\";\n}\n", &theme());
        assert_eq!(lines.len(), 3);
        let styles = styles(&lines);
        assert!(styles.iter().collect::<std::collections::HashSet<_>>().len() > 1, "{styles:?}");
    }

    #[test]
    fn a_language_nobody_bundles_falls_back_to_one_unstyled_span_a_line() {
        let lines = highlight(Some("nothing-of-the-sort"), "alpha\nbeta\n", &theme());
        assert_eq!(lines.len(), 2);
        assert_eq!(styles(&lines), vec![Style::default(), Style::default()]);
    }

    #[test]
    fn a_fence_with_no_language_at_all_is_still_laid_out_line_by_line() {
        let lines = highlight(None, "alpha\nbeta\ngamma\n", &theme());
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn the_same_block_twice_is_the_same_allocation_rather_than_a_second_highlight() {
        let first = highlight(Some("rust"), "fn cached() {}\n", &theme());
        let second = highlight(Some("rust"), "fn cached() {}\n", &theme());
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn the_language_is_part_of_the_key_so_two_readings_of_one_text_do_not_collide() {
        let source = "let x = 1\n";
        assert!(!Arc::ptr_eq(&highlight(Some("rust"), source, &theme()), &highlight(Some("swift"), source, &theme())));
    }

    #[test]
    fn a_trailing_newline_does_not_add_an_empty_line_at_the_end() {
        assert_eq!(highlight(Some("rust"), "fn main() {}", &theme()).len(), 1);
        assert_eq!(highlight(Some("rust"), "fn main() {}\n", &theme()).len(), 1);
        assert_eq!(highlight(Some("rust"), "fn main() {}\n\n", &theme()).len(), 1);
    }

    #[test]
    fn an_empty_block_is_no_lines_rather_than_one_blank_one() {
        assert!(highlight(Some("rust"), "", &theme()).is_empty());
        assert!(highlight(Some("rust"), "\n\n", &theme()).is_empty());
    }

    #[test]
    fn the_cache_evicts_rather_than_growing_without_a_bound() {
        for index in 0..CAPACITY.saturating_add(16) {
            highlight(Some("rust"), &format!("fn evicted{index}() {{}}\n"), &theme());
        }
        assert!(cache().lock().unwrap_or_else(PoisonError::into_inner).order.len() <= CAPACITY);
    }
}
