//! Fractional column widths, with a minimum, and the narrow-terminal collapse.

use ratatui::layout::Size;

use crate::ui::app::{CHROME_ROWS, Mode};

const MIN_WIDTH: u16 = 12;
const SHARES: [u16; 3] = [1, 1, 2];

pub fn narrow() -> u16 {
    let total = SHARES.iter().fold(0u16, |sum, &share| sum.saturating_add(share));
    let dividers = u16::try_from(SHARES.len()).unwrap_or(u16::MAX).saturating_sub(1);
    MIN_WIDTH.saturating_mul(total).saturating_add(dividers)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Columns {
    Three { projects: u16, sessions: u16, conversation: u16 },
    Two { sessions: u16, conversation: u16 },
}

pub fn layout(width: u16) -> Columns {
    if width < narrow() {
        let (_, narrow_shares) = SHARES.split_at_checked(1).unwrap_or((&SHARES, &[]));
        let widths = split(width, narrow_shares);
        let sessions = widths.first().copied().unwrap_or(0);
        let conversation = widths.get(1).copied().unwrap_or(0);
        return Columns::Two { sessions, conversation };
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
        Columns::Three { conversation, .. } | Columns::Two { conversation, .. } => conversation,
    }
}

pub fn conversation_height(area: Size) -> usize {
    usize::from(area.height.saturating_sub(CHROME_ROWS)).max(1)
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
    fn a_narrow_terminal_drops_the_projects_column() {
        assert!(matches!(layout(narrow().saturating_sub(1)), Columns::Two { .. }));
        assert!(matches!(layout(40), Columns::Two { .. }));
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
        assert!(matches!(layout(narrow().saturating_sub(1)), Columns::Two { .. }));
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
                Columns::Two { sessions, conversation } => {
                    let total = sessions.saturating_add(conversation);
                    assert!(total <= width.max(2), "{width} -> overflow");
                }
            }
        }
    }

    #[test]
    fn a_zero_width_terminal_does_not_panic() {
        let _ = layout(0);
    }

    #[test]
    fn the_conversation_height_matches_the_rows_a_list_column_gets() {
        let area = Size::new(60, 24);
        assert_eq!(conversation_height(area), usize::from(area.height.saturating_sub(CHROME_ROWS)));
    }
}
