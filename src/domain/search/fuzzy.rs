//! The `/` list filter's subsequence scorer.
//!
//! Deliberately not `nucleo-matcher`: that engine is built for identifiers and will happily
//! match `r…e…w…i…n…d` scattered across a whole paragraph of prose. A list of project paths
//! and session titles needs something closer to "these letters, roughly together, roughly
//! at the front" — about fifty lines gets there.

pub fn score(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }

    let mut needle_chars = needle.chars().flat_map(char::to_lowercase);
    let mut wanted = needle_chars.next();
    let mut total: i32 = 0;
    let mut streak: i32 = 0;

    for hay_char in haystack.chars().flat_map(char::to_lowercase) {
        let Some(want) = wanted else { break };
        if hay_char == want {
            streak = streak.saturating_add(1);
            total = total.saturating_add(streak);
            wanted = needle_chars.next();
        } else {
            streak = 0;
        }
    }

    wanted.is_none().then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_needle_matches_everything_at_a_zero_score() {
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn the_letters_must_appear_in_order() {
        assert!(score("grd", "grid").is_some());
        assert!(score("dg", "grid").is_none());
    }

    #[test]
    fn characters_out_of_order_never_match() {
        assert!(score("dr", "rewind").is_none());
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(score("REW", "rewind").is_some());
        assert!(score("rew", "Rewind").is_some());
    }

    #[test]
    fn a_contiguous_run_scores_higher_than_a_scattered_one() {
        let contiguous = score("grid", "grid scanner").expect("a match");
        let scattered = score("grid", "g-r-e-a-t indeed").expect("a match");
        assert!(contiguous > scattered, "contiguous={contiguous} scattered={scattered}");
    }

    #[test]
    fn a_needle_longer_than_the_haystack_cannot_match() {
        assert!(score("scanner", "grid").is_none());
    }
}
