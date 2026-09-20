//! Fractional column widths, with a minimum, and the narrow-terminal collapse.

use ratatui::layout::Size;

use crate::ui::app::{CHROME_ROWS, Column, Mode};

const MIN_WIDTH: u16 = 12;
const SHARES: [u16; 3] = [1, 1, 2];
const CONTENT_TOP: u16 = 3;

pub fn narrow() -> u16 {
    let total = SHARES.iter().fold(0u16, |sum, &share| sum.saturating_add(share));
    let dividers = u16::try_from(SHARES.len()).unwrap_or(u16::MAX).saturating_sub(1);
    MIN_WIDTH.saturating_mul(total).saturating_add(dividers)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Columns {
    Three { projects: u16, sessions: u16, conversation: u16 },
    Single { width: u16 },
}

pub fn layout(width: u16) -> Columns {
    if width < narrow() {
        return Columns::Single { width };
    }
    let widths = split(width, &SHARES);
    let projects = widths.first().copied().unwrap_or(0);
    let sessions = widths.get(1).copied().unwrap_or(0);
    let conversation = widths.get(2).copied().unwrap_or(0);
    Columns::Three { projects, sessions, conversation }
}

pub fn conversation_width(area: Size, mode: Mode) -> u16 {
    if mode == Mode::Focus {
        return area.width;
    }
    match layout(area.width) {
        Columns::Three { conversation, .. } => conversation,
        Columns::Single { width } => width,
    }
}

pub fn conversation_height(area: Size) -> usize {
    usize::from(area.height.saturating_sub(CHROME_ROWS)).max(1)
}

pub fn placement(width: u16, mode: Mode, focused: Column) -> Vec<(Column, u16, u16)> {
    if mode == Mode::Focus {
        return vec![(Column::Conversation, 0, width)];
    }
    match layout(width) {
        Columns::Three { projects, sessions, conversation } => {
            let x1 = projects;
            let x2 = x1.saturating_add(1).saturating_add(sessions);
            vec![
                (Column::Projects, 0, projects),
                (Column::Sessions, x1.saturating_add(1), sessions),
                (Column::Conversation, x2.saturating_add(1), conversation),
            ]
        }
        Columns::Single { width } => vec![(focused, 0, width)],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub column: Column,
    pub row: usize,
}

pub fn hit(area: Size, mode: Mode, focused: Column, x: u16, y: u16) -> Option<Hit> {
    if y < CONTENT_TOP {
        return None;
    }
    let row = usize::from(y.saturating_sub(CONTENT_TOP));
    let height = usize::from(area.height.saturating_sub(CHROME_ROWS)).max(1);
    if row >= height {
        return None;
    }
    placement(area.width, mode, focused)
        .into_iter()
        .find(|&(_, start, width)| x >= start && x < start.saturating_add(width))
        .map(|(column, _, _)| Hit { column, row })
}

fn split(width: u16, shares: &[u16]) -> Vec<u16> {
    let count = u16::try_from(shares.len()).unwrap_or(u16::MAX);
    let total_shares: u32 = shares.iter().map(|&share| u32::from(share)).sum();
    let available = width.saturating_sub(count.saturating_sub(1));
    let floor = MIN_WIDTH.min(available.checked_div(count).unwrap_or(0)).max(1);

    let mut widths: Vec<u16> = shares
        .iter()
        .map(|&share| {
            let natural = u32::from(available).saturating_mul(u32::from(share)).checked_div(total_shares.max(1)).unwrap_or(0);
            u16::try_from(natural).unwrap_or(u16::MAX).max(floor)
        })
        .collect();

    let total: u16 = widths.iter().fold(0u16, |sum, &value| sum.saturating_add(value));
    if total > available {
        let mut overflow = total.saturating_sub(available);
        while overflow > 0 {
            let Some((index, _)) = widths.iter().enumerate().max_by_key(|&(_, &value)| value) else { break };
            if widths.get(index).is_none_or(|&value| value <= floor) {
                break;
            }
            if let Some(value) = widths.get_mut(index) {
                *value = value.saturating_sub(1);
            }
            overflow = overflow.saturating_sub(1);
        }
    }
    widths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wide_terminal_shows_three_columns() {
        assert!(matches!(layout(narrow()), Columns::Three { .. }));
        assert!(matches!(layout(200), Columns::Three { .. }));
    }

    #[test]
    fn a_narrow_terminal_shows_one_pane_at_a_time() {
        assert!(matches!(layout(narrow().saturating_sub(1)), Columns::Single { .. }));
        assert!(matches!(layout(40), Columns::Single { .. }));
    }

    #[test]
    fn a_common_tmux_pane_width_keeps_all_three_columns() {
        for width in 80..=95u16 {
            assert!(matches!(layout(width), Columns::Three { .. }), "{width} columns should keep the projects column");
        }
    }

    #[test]
    fn the_threshold_is_the_width_at_which_every_share_still_clears_the_minimum() {
        assert_eq!(
            layout(narrow()),
            Columns::Three { projects: MIN_WIDTH, sessions: MIN_WIDTH, conversation: MIN_WIDTH.saturating_mul(2) }
        );
        assert!(matches!(layout(narrow().saturating_sub(1)), Columns::Single { .. }));
    }

    #[test]
    fn the_conversation_column_is_the_widest_share() {
        let widths = layout(160);
        assert!(matches!(widths, Columns::Three { .. }), "160 columns should keep all three panes");
        let Columns::Three { projects, sessions, conversation } = widths else { return };
        assert!(conversation > sessions);
        assert!(conversation > projects);
    }

    #[test]
    fn the_columns_never_overflow_the_total_width() {
        for width in 0..=300u16 {
            match layout(width) {
                Columns::Three { projects, sessions, conversation } => {
                    let total = projects.saturating_add(sessions).saturating_add(conversation);
                    assert!(total <= width.max(3), "{width} -> overflow");
                }
                Columns::Single { width: pane } => {
                    assert!(pane <= width.max(1), "{width} -> overflow");
                }
            }
        }
    }

    #[test]
    fn a_zero_width_terminal_does_not_panic() {
        let _ = layout(0);
    }

    #[test]
    fn the_conversation_column_gets_the_full_width_on_a_narrow_terminal() {
        let area = Size::new(40, 24);
        assert_eq!(conversation_width(area, Mode::Browse), 40);
    }

    #[test]
    fn the_conversation_height_matches_the_rows_a_list_column_gets() {
        let area = Size::new(60, 24);
        assert_eq!(conversation_height(area), usize::from(area.height.saturating_sub(CHROME_ROWS)));
    }

    #[test]
    fn placement_in_focus_mode_is_the_conversation_alone() {
        let placed = placement(60, Mode::Focus, Column::Projects);
        assert_eq!(placed, vec![(Column::Conversation, 0, 60)]);
    }

    #[test]
    fn placement_lists_three_columns_left_to_right_with_a_divider_gap() {
        let placed = placement(120, Mode::Browse, Column::Projects);
        let Columns::Three { projects, sessions, conversation } = layout(120) else {
            panic!("120 columns should keep three panes")
        };
        assert_eq!(
            placed,
            vec![
                (Column::Projects, 0, projects),
                (Column::Sessions, projects.saturating_add(1), sessions),
                (Column::Conversation, projects.saturating_add(1).saturating_add(sessions).saturating_add(1), conversation),
            ]
        );
    }

    #[test]
    fn placement_shows_only_the_focused_column_on_a_narrow_terminal() {
        for column in [Column::Projects, Column::Sessions, Column::Conversation] {
            assert_eq!(placement(40, Mode::Browse, column), vec![(column, 0, 40)]);
        }
    }

    #[test]
    fn a_click_above_the_column_label_hits_nothing() {
        assert_eq!(hit(Size::new(120, 24), Mode::Browse, Column::Projects, 5, 0), None);
        assert_eq!(hit(Size::new(120, 24), Mode::Browse, Column::Projects, 5, 2), None);
    }

    #[test]
    fn a_click_on_a_divider_hits_nothing() {
        let Columns::Three { projects, .. } = layout(120) else { panic!("120 columns should keep three panes") };
        assert_eq!(hit(Size::new(120, 24), Mode::Browse, Column::Projects, projects, 3), None);
    }

    #[test]
    fn a_click_past_the_last_content_row_hits_nothing() {
        assert_eq!(hit(Size::new(120, 24), Mode::Browse, Column::Projects, 5, 22), None);
        assert_eq!(hit(Size::new(120, 24), Mode::Browse, Column::Projects, 5, 23), None);
    }

    #[test]
    fn a_click_resolves_to_the_column_and_row_under_the_pointer() {
        let Columns::Three { projects, sessions, .. } = layout(120) else { panic!("120 columns should keep three panes") };
        assert_eq!(hit(Size::new(120, 24), Mode::Browse, Column::Projects, 5, 3), Some(Hit { column: Column::Projects, row: 0 }));
        assert_eq!(
            hit(Size::new(120, 24), Mode::Browse, Column::Projects, projects.saturating_add(2), 5),
            Some(Hit { column: Column::Sessions, row: 2 })
        );
        let conversation_x = projects.saturating_add(1).saturating_add(sessions).saturating_add(1);
        assert_eq!(
            hit(Size::new(120, 24), Mode::Browse, Column::Projects, conversation_x, 3),
            Some(Hit { column: Column::Conversation, row: 0 })
        );
    }

    #[test]
    fn a_click_on_a_narrow_terminal_always_hits_the_focused_column() {
        assert_eq!(hit(Size::new(40, 24), Mode::Browse, Column::Sessions, 0, 3), Some(Hit { column: Column::Sessions, row: 0 }));
        assert_eq!(hit(Size::new(40, 24), Mode::Browse, Column::Sessions, 39, 3), Some(Hit { column: Column::Sessions, row: 0 }));
    }

    #[test]
    fn a_click_in_focus_mode_always_hits_the_conversation() {
        assert_eq!(
            hit(Size::new(60, 24), Mode::Focus, Column::Projects, 0, 3),
            Some(Hit { column: Column::Conversation, row: 0 })
        );
        assert_eq!(
            hit(Size::new(60, 24), Mode::Focus, Column::Projects, 59, 3),
            Some(Hit { column: Column::Conversation, row: 0 })
        );
    }

    #[test]
    fn a_click_on_a_zero_size_terminal_does_not_panic() {
        assert_eq!(hit(Size::new(0, 0), Mode::Browse, Column::Projects, 0, 0), None);
    }
}
