//! Relative ages (`2d`, `1w`, `6mo`), rendered from an injected `now` so they stay testable.

use jiff::Timestamp;

const MINUTE: i64 = 60;
const HOUR: i64 = MINUTE * 60;
const DAY: i64 = HOUR * 24;
const WEEK: i64 = DAY * 7;
const MONTH: i64 = DAY * 30;
const YEAR: i64 = DAY * 365;

pub fn relative(now: Timestamp, at: Timestamp) -> String {
    let seconds = now.as_second().saturating_sub(at.as_second()).max(0);
    match seconds {
        0..MINUTE => "now".to_owned(),
        MINUTE..HOUR => format!("{}m", seconds.saturating_div(MINUTE)),
        HOUR..DAY => format!("{}h", seconds.saturating_div(HOUR)),
        DAY..WEEK => format!("{}d", seconds.saturating_div(DAY)),
        WEEK..MONTH => format!("{}w", seconds.saturating_div(WEEK)),
        MONTH..YEAR => format!("{}mo", seconds.saturating_div(MONTH)),
        _ => format!("{}y", seconds.saturating_div(YEAR)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        text.parse().expect("a valid instant")
    }

    #[test]
    fn under_a_minute_reads_as_now() {
        let now = at("2026-01-05T12:00:30Z");
        assert_eq!(relative(now, at("2026-01-05T12:00:00Z")), "now");
    }

    #[test]
    fn minutes_and_hours_are_counted() {
        let now = at("2026-01-05T12:00:00Z");
        assert_eq!(relative(now, at("2026-01-05T11:55:00Z")), "5m");
        assert_eq!(relative(now, at("2026-01-05T09:00:00Z")), "3h");
    }

    #[test]
    fn days_and_weeks_match_the_readme_mockup() {
        let now = at("2026-01-12T00:00:00Z");
        assert_eq!(relative(now, at("2026-01-10T00:00:00Z")), "2d");
        assert_eq!(relative(now, at("2026-01-05T00:00:00Z")), "1w");
    }

    #[test]
    fn months_and_years_are_counted() {
        let now = at("2026-07-05T00:00:00Z");
        assert_eq!(relative(now, at("2026-01-05T00:00:00Z")), "6mo");
        assert_eq!(relative(now, at("2024-01-05T00:00:00Z")), "2y");
    }

    #[test]
    fn a_timestamp_in_the_future_never_goes_negative() {
        let now = at("2026-01-05T00:00:00Z");
        assert_eq!(relative(now, at("2026-01-06T00:00:00Z")), "now");
    }
}
