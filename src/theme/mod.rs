//! Themes: a palette of semantic colors and the element styles from it.

pub mod color;
pub mod elements;
pub mod palette;

use ratatui::style::Style;

pub use elements::Element;
pub use palette::Palette;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub palette: Palette,
    styles: Vec<Style>,
    pub name: String,
    pub syntax_theme: Option<String>,
}

impl Theme {
    pub fn new(palette: Palette) -> Self {
        let styles = Element::ALL.iter().map(|element| elements::default_style(*element, &palette)).collect();
        Self { palette, styles, name: String::from("ansi"), syntax_theme: None }
    }

    pub fn style(&self, element: Element) -> Style {
        self.styles.get(element.index()).copied().unwrap_or_default()
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::new(Palette::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    #[test]
    fn every_element_is_listed_once_in_discriminant_order() {
        for (index, element) in Element::ALL.iter().enumerate() {
            assert_eq!(element.index(), index, "{element:?} is out of order in Element::ALL");
        }
    }

    #[test]
    fn styles_are_looked_up_by_element() {
        let theme = Theme::default();
        assert_eq!(theme.style(Element::Heading).fg, None);
        assert!(theme.style(Element::Heading).add_modifier.contains(Modifier::BOLD));
        assert_eq!(theme.style(Element::InlineCode).bg, None);
        assert_eq!(theme.style(Element::CursorLine).bg, Some(theme.palette.cursor));
    }

    #[test]
    fn a_repalette_moves_every_element_derived_from_that_slot() {
        let palette = Palette { accent: Color::Rgb(1, 2, 3), ..Palette::default() };
        let theme = Theme::new(palette);
        assert_eq!(theme.style(Element::HumanGutter).fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(theme.style(Element::InlineCode).fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(theme.style(Element::ScrollProgress).fg, Some(Color::Rgb(1, 2, 3)));
    }

    #[test]
    fn the_human_and_assistant_gutters_are_two_different_colours() {
        let theme = Theme::default();
        assert_ne!(theme.style(Element::HumanGutter).fg, theme.style(Element::AssistantGutter).fg);
    }

    #[test]
    fn emphasis_and_strong_carry_no_color_of_their_own() {
        let theme = Theme::default();
        assert_eq!(theme.style(Element::Emphasis).fg, None);
        assert_eq!(theme.style(Element::Strong).fg, None);
    }
}
