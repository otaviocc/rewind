//! A theme file, the chain it inherits through, and the `Theme` it resolves to.
//!
//! Everything a file can state is `Option`, so a two-line theme is a valid one: what it does
//! not say falls through to the base, and past the base to the defaults in Rust.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use ratatui::style::{Modifier, Style};
use serde::Deserialize;
use thiserror::Error;

use crate::theme::color::ColorError;
use crate::theme::elements::{self, ElementFile};
use crate::theme::palette::PaletteFile;
use crate::theme::{Element, Palette, Theme};

const ANSI: &str = include_str!("../../themes/ansi.toml");

pub const BUILT_INS: &[(&str, &str)] = &[("ansi", ANSI)];

pub const IMPLICIT_BASE: &str = "ansi";

#[derive(Debug, Error)]
pub enum ThemeError {
    #[error("cannot read {origin}")]
    Unreadable {
        origin: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{origin} is not a valid theme")]
    Malformed {
        origin: String,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("{key} is not a color")]
    BadColor {
        key: String,
        #[source]
        source: ColorError,
    },
    #[error("{key}: {value:?} is not a modifier")]
    BadModifier { key: String, value: String },
    #[error("there is no theme called {0:?}")]
    Unknown(String),
    #[error("the base chain loops: {0}")]
    Cycle(String),
}

#[derive(Debug, Default, Deserialize)]
pub struct ThemeFile {
    pub base: Option<String>,
    pub name: Option<String>,
    pub syntax_theme: Option<String>,
    #[serde(default)]
    pub palette: PaletteFile,
    #[serde(default)]
    pub elements: BTreeMap<String, ElementFile>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, toml::Value>,
}

impl ThemeFile {
    pub fn parse(text: &str, origin: &str) -> Result<Self, ThemeError> {
        toml::from_str(text).map_err(|source| ThemeError::Malformed { origin: origin.to_owned(), source: Box::new(source) })
    }

    #[must_use]
    pub fn merge(self, base: Self) -> Self {
        let mut elements = base.elements;
        for (key, child) in self.elements {
            let merged = match elements.remove(&key) {
                Some(existing) => child.merge(existing),
                None => child,
            };
            elements.insert(key, merged);
        }

        let mut unknown = base.unknown;
        unknown.extend(self.unknown);

        Self {
            base: base.base,
            name: self.name,
            syntax_theme: self.syntax_theme.or(base.syntax_theme),
            palette: self.palette.merge(base.palette),
            elements,
            unknown,
        }
    }

    pub fn warnings(&self, origin: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        for key in self.unknown.keys() {
            warnings.push(format!("{origin}: {key} is not a theme key"));
        }
        for key in self.palette.unknown.keys() {
            warnings.push(format!("{origin}: palette.{key} is not a palette slot"));
        }
        for (key, element) in &self.elements {
            if Element::from_key(key).is_none() {
                warnings.push(format!("{origin}: elements.{key} is not an element"));
            }
            for unknown in element.unknown.keys() {
                warnings.push(format!("{origin}: elements.{key}.{unknown} is not an element key"));
            }
        }
        warnings
    }
}

#[derive(Debug)]
pub struct Loaded {
    pub theme: Theme,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    BuiltIn(&'static str, &'static str),
    File(PathBuf),
}

impl Source {
    fn origin(&self) -> String {
        match self {
            Self::BuiltIn(name, _) => format!("built-in theme {name}"),
            Self::File(path) => path.display().to_string(),
        }
    }

    fn identity(&self) -> String {
        match self {
            Self::BuiltIn(name, _) => format!("built-in:{name}"),
            Self::File(path) => {
                let path = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                format!("file:{}", path.display())
            }
        }
    }

    fn read(&self) -> Result<ThemeFile, ThemeError> {
        let origin = self.origin();
        let text = match self {
            Self::BuiltIn(_, text) => (*text).to_owned(),
            Self::File(path) => {
                fs::read_to_string(path).map_err(|source| ThemeError::Unreadable { origin: origin.clone(), source })?
            }
        };
        ThemeFile::parse(&text, &origin)
    }
}

fn nameable(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
}

fn user_theme(config_dir: Option<&Path>, name: &str) -> Option<PathBuf> {
    let path = config_dir?.join("themes").join(format!("{name}.toml"));
    path.is_file().then_some(path)
}

fn find(name: &str, config_dir: Option<&Path>) -> Result<Source, ThemeError> {
    if !nameable(name) {
        return Err(ThemeError::Unknown(name.to_owned()));
    }
    if let Some(path) = user_theme(config_dir, name) {
        return Ok(Source::File(path));
    }
    BUILT_INS
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(key, text)| Source::BuiltIn(key, text))
        .ok_or_else(|| ThemeError::Unknown(name.to_owned()))
}

fn head(config: Option<&Path>, name: Option<&str>, config_dir: Option<&Path>) -> Result<Option<(Source, String)>, ThemeError> {
    if let Some(path) = config {
        let label = path.file_stem().map_or_else(|| String::from("theme"), |stem| stem.to_string_lossy().into_owned());
        return Ok(Some((Source::File(path.to_path_buf()), label)));
    }
    if let Some(name) = name {
        return Ok(Some((find(name, config_dir)?, name.to_owned())));
    }
    if let Some(dir) = config_dir {
        let path = dir.join("theme.toml");
        if path.is_file() {
            return Ok(Some((Source::File(path), String::from("theme"))));
        }
    }
    Ok(None)
}

pub fn load(config: Option<&Path>, name: Option<&str>, config_dir: Option<&Path>) -> Result<Loaded, ThemeError> {
    let Some((source, label)) = head(config, name, config_dir)? else {
        return Ok(Loaded { theme: Theme::default(), warnings: Vec::new() });
    };

    let mut warnings = Vec::new();
    let mut seen = BTreeSet::new();
    let mut chain = vec![label.clone()];

    let mut merged = source.read()?;
    warnings.extend(merged.warnings(&source.origin()));
    seen.insert(source.identity());

    loop {
        let next = if let Some(base) = merged.base.take() {
            let source = find(&base, config_dir)?;
            chain.push(base);
            if !seen.insert(source.identity()) {
                return Err(ThemeError::Cycle(chain.join(" -> ")));
            }
            source
        } else {
            let implicit = find(IMPLICIT_BASE, config_dir)?;
            if !seen.insert(implicit.identity()) {
                break;
            }
            implicit
        };
        let file = next.read()?;
        warnings.extend(file.warnings(&next.origin()));
        merged = merged.merge(file);
    }

    Ok(Loaded { theme: resolve(merged, &label)?, warnings })
}

pub fn resolve(file: ThemeFile, label: &str) -> Result<Theme, ThemeError> {
    let palette = palette(&file)?;
    let mut theme = Theme::new(palette);

    for (key, element) in &file.elements {
        let Some(which) = Element::from_key(key) else {
            continue;
        };
        theme.set_style(which, style(theme.style(which), element, &palette, key)?);
    }

    theme.name = file.name.unwrap_or_else(|| label.to_owned());
    theme.syntax_theme = file.syntax_theme;
    Ok(theme)
}

fn palette(file: &ThemeFile) -> Result<Palette, ThemeError> {
    let mut palette = Palette::default();
    for (slot, spec) in file.palette.slots() {
        let Some(spec) = spec else { continue };
        let color = spec.resolve().map_err(|source| ThemeError::BadColor { key: format!("palette.{slot}"), source })?;
        PaletteFile::set(&mut palette, slot, color);
    }
    if file.palette.cursor.is_none() {
        palette.cursor = palette.subtle;
    }
    Ok(palette)
}

fn style(base: Style, file: &ElementFile, palette: &Palette, key: &str) -> Result<Style, ThemeError> {
    let mut style = base;

    if let Some(names) = &file.modifiers {
        style.add_modifier = Modifier::empty();
        style.sub_modifier = Modifier::empty();
        for name in names {
            let modifier = elements::modifier(name)
                .ok_or_else(|| ThemeError::BadModifier { key: format!("elements.{key}.modifiers"), value: name.clone() })?;
            style = style.add_modifier(modifier);
        }
    }

    if let Some(spec) = &file.bg {
        if spec.removes_color() {
            style.bg = None;
        } else {
            let color = spec
                .resolve_against(palette)
                .map_err(|source| ThemeError::BadColor { key: format!("elements.{key}.bg"), source })?;
            style = style.bg(color);
        }
    }

    if let Some(spec) = &file.fg {
        let color =
            spec.resolve_against(palette).map_err(|source| ThemeError::BadColor { key: format!("elements.{key}.fg"), source })?;
        style = style.fg(color);
    }

    Ok(style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn parse(text: &str) -> ThemeFile {
        ThemeFile::parse(text, "a test theme").expect("the theme parses")
    }

    fn theme(text: &str) -> Theme {
        resolve(parse(text), "test").expect("the theme resolves")
    }

    fn error(text: &str) -> ThemeError {
        resolve(parse(text), "test").expect_err("the theme is rejected")
    }

    #[test]
    fn the_whole_file_shape_parses_through_nested_flattening() {
        let file = parse(
            "base = \"ansi\"\nname = \"mine\"\nsyntax_theme = \"base16-ocean.dark\"\nstray = 1\n\n[palette]\naccent = \"#89b4fa\"\nnotice = 208\nforground = \"red\"\n\n[elements.body]\nfg = \"accent\"\nbg = \"none\"\nmodifiers = [\"bold\"]\ncolour = \"red\"\n",
        );
        assert_eq!(file.base.as_deref(), Some("ansi"));
        assert_eq!(file.name.as_deref(), Some("mine"));
        assert_eq!(file.syntax_theme.as_deref(), Some("base16-ocean.dark"));
        assert_eq!(file.palette.notice, Some(crate::theme::color::ColorSpec::Index(208)), "an integer lost its type");
        assert!(file.palette.unknown.contains_key("forground"), "the palette typo was not captured");
        assert!(file.unknown.contains_key("stray"), "the top-level typo was not captured");
        assert!(!file.unknown.contains_key("palette"), "a named table leaked into the unknown map");
        assert!(file.elements.get("body").expect("the body element").unknown.contains_key("colour"));
    }

    #[test]
    fn a_two_line_user_theme_inherits_every_slot_it_does_not_state() {
        let theme = theme("[palette]\naccent = \"#89b4fa\"\n");
        assert_eq!(theme.palette.accent, Color::Rgb(0x89, 0xB4, 0xFA), "the slot the file stated did not take");
        let defaults = Palette::default();
        assert_eq!(theme.palette.error, defaults.error);
        assert_eq!(theme.palette.notice, defaults.notice);
        assert_eq!(theme.palette.background, defaults.background, "a partial file repainted the terminal's ground");
    }

    #[test]
    fn an_empty_file_is_the_defaults_which_is_what_makes_ansi_the_implicit_base() {
        let theme = theme("");
        assert_eq!(theme.palette, Palette::default());
        assert_eq!(theme.style(Element::Heading), Theme::default().style(Element::Heading));
    }

    #[test]
    fn a_child_wins_its_own_fields_and_keeps_the_rest_of_the_base() {
        let merged = parse("[palette]\naccent = \"red\"\n").merge(parse("[palette]\naccent = \"blue\"\nnotice = \"green\"\n"));
        let resolved = resolve(merged, "test").expect("the merged theme resolves");
        assert_eq!(resolved.palette.accent, Color::Red, "the child did not win its own slot");
        assert_eq!(resolved.palette.notice, Color::Green, "the base's slot was dropped");
    }

    #[test]
    fn an_element_merges_field_by_field_rather_than_wholesale() {
        let merged = parse("[elements.body]\nbg = \"red\"\n").merge(parse("[elements.body]\nfg = \"green\"\n"));
        let resolved = resolve(merged, "test").expect("the merged theme resolves");
        assert_eq!(resolved.style(Element::Body).bg, Some(Color::Red));
        assert_eq!(resolved.style(Element::Body).fg, Some(Color::Green), "the base's foreground was dropped");
    }

    #[test]
    fn merging_carries_the_parents_base_forward_so_a_chain_can_keep_walking() {
        let merged = parse("base = \"nord\"\n").merge(parse("base = \"ansi\"\n"));
        assert_eq!(merged.base.as_deref(), Some("ansi"), "the link still to follow was dropped");

        let merged = parse("base = \"nord\"\n").merge(parse("[palette]\naccent = \"red\"\n"));
        assert_eq!(merged.base, None, "a chain that has reached its end kept a link");
    }

    #[test]
    fn a_theme_that_inherits_from_another_is_not_renamed_by_it() {
        let merged = parse("[palette]\naccent = \"red\"\n").merge(parse("name = \"nord\"\n"));
        let resolved = resolve(merged, "mine").expect("the merged theme resolves");
        assert_eq!(resolved.name, "mine", "inheriting from a theme renamed this one after it");
    }

    #[test]
    fn a_syntax_theme_is_inherited_because_code_should_match_the_base() {
        let merged = parse("[palette]\naccent = \"red\"\n").merge(parse("syntax_theme = \"solarized\"\n"));
        let resolved = resolve(merged, "mine").expect("the merged theme resolves");
        assert_eq!(resolved.syntax_theme.as_deref(), Some("solarized"));
    }

    #[test]
    fn cursor_falls_back_to_subtle_when_the_chain_never_sets_it() {
        let theme = theme("[palette]\nsubtle = \"#101010\"\n");
        assert_eq!(theme.palette.cursor, Color::Rgb(0x10, 0x10, 0x10), "cursor did not follow subtle");
        assert_eq!(theme.style(Element::CursorLine).bg, Some(Color::Rgb(0x10, 0x10, 0x10)));
    }

    #[test]
    fn cursor_stays_where_a_theme_that_states_it_put_it() {
        let theme = theme("[palette]\nsubtle = \"#101010\"\ncursor = \"#202020\"\n");
        assert_eq!(theme.palette.cursor, Color::Rgb(0x20, 0x20, 0x20), "an explicit cursor was overridden by subtle");
    }

    #[test]
    fn modifiers_replace_the_defaults_rather_than_adding_to_them() {
        assert!(Theme::default().style(Element::Heading).add_modifier.contains(Modifier::BOLD));
        let theme = theme("[elements.heading]\nmodifiers = [\"italic\"]\n");
        let style = theme.style(Element::Heading);
        assert!(style.add_modifier.contains(Modifier::ITALIC));
        assert!(!style.add_modifier.contains(Modifier::BOLD), "the default modifier was added to rather than replaced");
    }

    #[test]
    fn an_empty_modifier_list_clears_the_defaults() {
        let theme = theme("[elements.heading]\nmodifiers = []\n");
        assert_eq!(theme.style(Element::Heading).add_modifier, Modifier::empty(), "an empty list did not unbold");
    }

    #[test]
    fn a_background_of_none_removes_the_one_the_default_gave() {
        assert!(Theme::default().style(Element::CursorLine).bg.is_some());
        assert_eq!(theme("[elements.cursor_line]\nbg = \"none\"\n").style(Element::CursorLine).bg, None);
    }

    #[test]
    fn an_element_may_name_a_palette_slot_and_the_slot_wins_over_a_literal() {
        let theme = theme("[palette]\naccent = \"#89b4fa\"\n\n[elements.body]\nfg = \"accent\"\n");
        assert_eq!(theme.style(Element::Body).fg, Some(Color::Rgb(0x89, 0xB4, 0xFA)));
    }

    #[test]
    fn a_bad_palette_color_names_the_slot_that_carried_it() {
        let error = error("[palette]\naccent = \"blurple\"\n");
        assert!(matches!(error, ThemeError::BadColor { .. }));
        let message = format!("{error}");
        assert!(message.contains("palette.accent"), "{message}");
        assert!(format!("{}", anyhow::Error::from(error)).contains("palette.accent"));
    }

    #[test]
    fn a_bad_element_color_names_the_element_and_the_field() {
        let message = format!("{}", error("[elements.body]\nfg = \"blurple\"\n"));
        assert!(message.contains("elements.body.fg"), "{message}");
    }

    #[test]
    fn a_foreground_of_none_is_an_error_because_only_a_background_can_be_removed() {
        let message = format!("{}", error("[elements.body]\nfg = \"none\"\n"));
        assert!(message.contains("elements.body.fg"), "{message}");
    }

    #[test]
    fn a_bad_modifier_names_the_element_and_the_value() {
        let error = error("[elements.heading]\nmodifiers = [\"blinky\"]\n");
        assert!(matches!(error, ThemeError::BadModifier { .. }));
        let message = format!("{error}");
        assert!(message.contains("elements.heading.modifiers"), "{message}");
        assert!(message.contains("blinky"), "{message}");
    }

    #[test]
    fn a_malformed_file_names_the_file_it_could_not_read() {
        let error = ThemeFile::parse("[palette", "mine.toml").expect_err("the file is malformed");
        assert!(matches!(error, ThemeError::Malformed { .. }));
        assert!(format!("{error}").contains("mine.toml"), "{error}");
    }

    #[test]
    fn an_unknown_key_warns_and_names_where_it_came_from_while_the_theme_still_loads() {
        let file = parse("stray = 1\n\n[palette]\nforground = \"red\"\naccent = \"red\"\n\n[elements.body]\ncolour = \"red\"\n");
        let warnings = file.warnings("mine.toml");
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings.iter().all(|warning| warning.starts_with("mine.toml: ")), "{warnings:?}");
        assert!(warnings.iter().any(|warning| warning.contains("palette.forground")), "{warnings:?}");
        assert!(warnings.iter().any(|warning| warning.contains("elements.body.colour")), "{warnings:?}");
        assert_eq!(resolve(file, "mine").expect("the theme still loads").palette.accent, Color::Red);
    }

    #[test]
    fn an_unknown_element_name_warns_rather_than_failing_because_flattening_cannot_catch_it() {
        let file = parse("[elements.headings]\nfg = \"red\"\n");
        let warnings = file.warnings("mine.toml");
        assert_eq!(warnings, vec![String::from("mine.toml: elements.headings is not an element")]);
        resolve(file, "mine").expect("an unknown element name is not fatal");
    }

    fn config(themes: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::TempDir::new().expect("a temporary config directory");
        std::fs::create_dir_all(dir.path().join("themes")).expect("a themes directory");
        for (name, text) in themes {
            let path = if *name == "theme" {
                dir.path().join("theme.toml")
            } else {
                dir.path().join("themes").join(format!("{name}.toml"))
            };
            std::fs::write(path, text).expect("a written theme");
        }
        dir
    }

    fn loaded(dir: &tempfile::TempDir, name: Option<&str>) -> Loaded {
        load(None, name, Some(dir.path())).expect("the theme loads")
    }

    #[test]
    fn nothing_at_all_loads_the_built_in_default() {
        let loaded = load(None, None, None).expect("no configuration is not a failure");
        assert_eq!(loaded.theme, Theme::default());
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn a_two_line_theme_file_inherits_the_rest_through_the_implicit_ansi_base() {
        let dir = config(&[("theme", "[palette]\naccent = \"#89b4fa\"\n")]);
        let theme = loaded(&dir, None).theme;
        assert_eq!(theme.palette.accent, Color::Rgb(0x89, 0xB4, 0xFA));
        assert_eq!(theme.palette.notice, Palette::default().notice, "the implicit base did not fill the rest in");
        assert_eq!(theme.name, "theme");
    }

    #[test]
    fn a_base_chain_merges_each_child_over_the_one_it_names() {
        let dir = config(&[
            ("bottom", "[palette]\naccent = \"blue\"\nnotice = \"green\"\nerror = \"yellow\"\n"),
            ("middle", "base = \"bottom\"\n\n[palette]\nnotice = \"magenta\"\nerror = \"cyan\"\n"),
            ("top", "base = \"middle\"\n\n[palette]\nerror = \"red\"\n"),
        ]);
        let theme = loaded(&dir, Some("top")).theme;
        assert_eq!(theme.palette.error, Color::Red, "the child did not win");
        assert_eq!(theme.palette.notice, Color::Magenta, "the middle of the chain did not win over the bottom");
        assert_eq!(theme.palette.accent, Color::Blue, "the bottom of the chain was dropped");
        assert_eq!(theme.name, "top", "the chain renamed the theme after one of its bases");
    }

    #[test]
    fn a_base_that_points_back_at_the_child_is_a_cycle_and_not_a_stack_overflow() {
        let dir = config(&[("one", "base = \"two\"\n"), ("two", "base = \"one\"\n")]);
        let error = load(None, Some("one"), Some(dir.path())).expect_err("a loop is not a theme");
        assert!(matches!(error, ThemeError::Cycle(_)));
        let message = format!("{error}");
        assert!(message.contains("one -> two -> one"), "{message}");
    }

    #[test]
    fn a_theme_that_names_itself_as_its_base_is_a_cycle() {
        let dir = config(&[("mine", "base = \"mine\"\n")]);
        let error = load(None, Some("mine"), Some(dir.path())).expect_err("a self-reference is not a theme");
        assert!(matches!(error, ThemeError::Cycle(_)), "{error}");
    }

    #[test]
    fn the_implicit_ansi_base_is_not_a_cycle_when_the_chain_already_named_ansi() {
        let dir = config(&[("mine", "base = \"ansi\"\n\n[palette]\naccent = \"red\"\n")]);
        let theme = loaded(&dir, Some("mine")).theme;
        assert_eq!(theme.palette.accent, Color::Red, "an explicit ansi base was reported as a loop");
    }

    #[test]
    fn a_user_theme_shadows_a_built_in_of_the_same_name() {
        let dir = config(&[("ansi", "[palette]\naccent = \"red\"\n")]);
        let theme = loaded(&dir, Some("ansi")).theme;
        assert_eq!(theme.palette.accent, Color::Red, "the built-in was used instead of the user file");
    }

    #[test]
    fn a_user_ansi_becomes_the_implicit_base_for_every_other_theme() {
        let dir = config(&[("ansi", "[palette]\nnotice = \"red\"\n"), ("mine", "[palette]\naccent = \"blue\"\n")]);
        let theme = loaded(&dir, Some("mine")).theme;
        assert_eq!(theme.palette.accent, Color::Blue);
        assert_eq!(theme.palette.notice, Color::Red, "the shadowed ansi was not used as the implicit base");
    }

    #[test]
    fn an_explicit_config_file_outranks_a_theme_name() {
        let dir = config(&[("mine", "[palette]\naccent = \"blue\"\n")]);
        let explicit = dir.path().join("explicit.toml");
        std::fs::write(&explicit, "[palette]\naccent = \"red\"\n").expect("a written theme");
        let loaded = load(Some(&explicit), Some("mine"), Some(dir.path())).expect("the theme loads");
        assert_eq!(loaded.theme.palette.accent, Color::Red, "--theme won over --config");
        assert_eq!(loaded.theme.name, "explicit");
    }

    #[test]
    fn a_theme_name_outranks_the_config_directory_theme_file() {
        let dir = config(&[("theme", "[palette]\naccent = \"blue\"\n"), ("mine", "[palette]\naccent = \"red\"\n")]);
        assert_eq!(loaded(&dir, Some("mine")).theme.palette.accent, Color::Red);
        assert_eq!(loaded(&dir, None).theme.palette.accent, Color::Blue);
    }

    #[test]
    fn an_unknown_theme_name_names_the_theme_that_was_asked_for() {
        let dir = config(&[]);
        let error = load(None, Some("nosuch"), Some(dir.path())).expect_err("there is no such theme");
        assert!(matches!(error, ThemeError::Unknown(_)));
        assert!(format!("{error}").contains("nosuch"), "{error}");
    }

    #[test]
    fn a_theme_name_that_is_a_path_is_not_a_theme() {
        let dir = config(&[]);
        for name in ["../../../etc/passwd", "a/b", "..", "", "a.toml"] {
            let error = load(None, Some(name), Some(dir.path())).expect_err("a path is not a theme name");
            assert!(matches!(error, ThemeError::Unknown(_)), "{name:?} was treated as a theme name");
        }
    }

    #[test]
    fn a_config_file_that_is_not_there_is_fatal_because_it_was_named() {
        let dir = config(&[]);
        let missing = dir.path().join("nosuch.toml");
        let error = load(Some(&missing), None, Some(dir.path())).expect_err("a named file must exist");
        assert!(matches!(error, ThemeError::Unreadable { .. }));
        assert!(format!("{error}").contains("nosuch.toml"), "{error}");
    }

    #[test]
    fn a_missing_config_directory_theme_file_is_not_an_error_at_all() {
        let dir = config(&[]);
        assert_eq!(loaded(&dir, None).theme, Theme::default());
    }

    #[test]
    fn a_warning_names_the_file_it_came_from_rather_than_the_merged_result() {
        let dir = config(&[("base-theme", "[palette]\nforground = \"red\"\n"), ("mine", "base = \"base-theme\"\nstray = 1\n")]);
        let warnings = loaded(&dir, Some("mine")).warnings;
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|warning| warning.contains("mine.toml") && warning.contains("stray")), "{warnings:?}");
        assert!(
            warnings.iter().any(|warning| warning.contains("base-theme.toml") && warning.contains("forground")),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_bad_color_in_a_base_the_child_overrides_is_not_fatal() {
        let dir = config(&[
            ("base-theme", "[palette]\naccent = \"blurple\"\n"),
            ("mine", "base = \"base-theme\"\n\n[palette]\naccent = \"red\"\n"),
        ]);
        assert_eq!(loaded(&dir, Some("mine")).theme.palette.accent, Color::Red, "an overridden bad colour was still fatal");
    }

    #[test]
    fn the_built_in_ansi_theme_parses_and_asserts_nothing() {
        let file = ThemeFile::parse(ANSI, "built-in theme ansi").expect("the built-in parses");
        assert!(file.warnings("ansi").is_empty(), "the built-in has a key nothing reads");
        let theme = resolve(file, "ansi").expect("the built-in resolves");
        assert_eq!(theme.palette, Palette::default(), "ansi asserts something the defaults do not");
        assert_eq!(theme.name, "ansi");
    }
}
