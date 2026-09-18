//! A theme file, the chain it inherits through, and the `Theme` it resolves to.
//!
//! Everything a file can state is `Option`, so a two-line theme is a valid one: what it does
//! not say falls through to the base, and past the base to the defaults in Rust.

use std::collections::BTreeMap;

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

    #[test]
    fn the_built_in_ansi_theme_parses_and_asserts_nothing() {
        let file = ThemeFile::parse(ANSI, "built-in theme ansi").expect("the built-in parses");
        assert!(file.warnings("ansi").is_empty(), "the built-in has a key nothing reads");
        let theme = resolve(file, "ansi").expect("the built-in resolves");
        assert_eq!(theme.palette, Palette::default(), "ansi asserts something the defaults do not");
        assert_eq!(theme.name, "ansi");
    }
}
