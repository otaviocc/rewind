//! Substring matching and match-quality scoring against the already-lowercased shard blob.
//!
//! Scores are fixed-point integers out of 100, not floats: this crate denies
//! `arithmetic_side_effects` and `as_conversions`, and every existing scoring-shaped
//! computation in the codebase (`ui::view::progress`, `listing`) already works this way.
//! It also sidesteps "case agreement" as a quality signal — the blob is lowercased at build
//! time, so by the time a query reaches the matcher neither side has case left to agree on.
//! Word boundaries, whole words and term proximity carry the weight instead.

use memchr::memmem::Finder;

const BASE_QUALITY: u32 = 40;
const BOUNDARY_BONUS: u32 = 30;
const MAX_QUALITY: u32 = 100;
const PROXIMITY_FLOOR: u32 = 40;
const PROXIMITY_SPAN_CAP: u64 = 400;
const PROXIMITY_FACTOR_FLOOR: u32 = 70;
const PROXIMITY_FACTOR_SPAN: u32 = 30;

pub struct Needles {
    positive: Vec<(usize, Finder<'static>)>,
    negative: Vec<Finder<'static>>,
}

impl Needles {
    pub fn build(positive: &[String], negative: &[String]) -> Self {
        let positive = positive.iter().map(|term| (term.len(), Finder::new(term.as_bytes()).into_owned())).collect();
        let negative = negative.iter().map(|term| Finder::new(term.as_bytes()).into_owned()).collect();
        Self { positive, negative }
    }

    pub const fn is_empty(&self) -> bool {
        self.positive.is_empty()
    }

    /// `None` when the haystack does not satisfy the query (a positive term is missing, or a
    /// negated one is present). `Some(quality)` otherwise, `quality` in `0..=100`.
    pub fn quality(&self, haystack: &[u8]) -> Option<u32> {
        if self.positive.is_empty() {
            return None;
        }
        if self.negative.iter().any(|finder| finder.find(haystack).is_some()) {
            return None;
        }

        let mut total: u32 = 0;
        let mut earliest: Vec<usize> = Vec::with_capacity(self.positive.len());
        for (len, finder) in &self.positive {
            let (best, first) = best_occurrence(haystack, finder, *len)?;
            total = total.saturating_add(best);
            earliest.push(first);
        }

        let count = u32::try_from(self.positive.len()).unwrap_or(u32::MAX).max(1);
        let average = total.checked_div(count).unwrap_or(0);
        let factor = proximity_factor(&earliest);
        Some(average.saturating_mul(factor).checked_div(MAX_QUALITY).unwrap_or(0).min(MAX_QUALITY))
    }
}

fn best_occurrence(haystack: &[u8], finder: &Finder<'_>, term_len: usize) -> Option<(u32, usize)> {
    let mut best: Option<u32> = None;
    let mut first: Option<usize> = None;
    for start in finder.find_iter(haystack) {
        let end = start.checked_add(term_len)?;
        let score = boundary_score(haystack, start, end);
        best = Some(best.map_or(score, |current| current.max(score)));
        if first.is_none() {
            first = Some(start);
        }
    }
    Some((best?, first?))
}

fn boundary_score(haystack: &[u8], start: usize, end: usize) -> u32 {
    let before = start.checked_sub(1).and_then(|index| haystack.get(index)).copied();
    let after = haystack.get(end).copied();
    let mut score = BASE_QUALITY;
    if is_boundary(before) {
        score = score.saturating_add(BOUNDARY_BONUS);
    }
    if is_boundary(after) {
        score = score.saturating_add(BOUNDARY_BONUS);
    }
    score.min(MAX_QUALITY)
}

fn is_boundary(byte: Option<u8>) -> bool {
    byte.is_none_or(|byte| !(byte.is_ascii_alphanumeric() || byte == b'_'))
}

fn proximity_factor(earliest: &[usize]) -> u32 {
    if earliest.len() < 2 {
        return MAX_QUALITY;
    }
    let min = earliest.iter().copied().min().unwrap_or(0);
    let max = earliest.iter().copied().max().unwrap_or(0);
    let span = u64::try_from(max.saturating_sub(min)).unwrap_or(u64::MAX).min(PROXIMITY_SPAN_CAP);
    let closeness = PROXIMITY_SPAN_CAP.saturating_sub(span);
    let closeness = u32::try_from(closeness.saturating_mul(100).checked_div(PROXIMITY_SPAN_CAP).unwrap_or(0)).unwrap_or(0);
    let closeness = closeness.max(PROXIMITY_FLOOR);
    PROXIMITY_FACTOR_FLOOR.saturating_add(closeness.saturating_mul(PROXIMITY_FACTOR_SPAN).checked_div(100).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn needles(positive: &[&str]) -> Needles {
        Needles::build(&positive.iter().map(|term| (*term).to_owned()).collect::<Vec<_>>(), &[])
    }

    #[test]
    fn a_missing_term_scores_nothing() {
        let needles = needles(&["scanner"]);
        assert_eq!(needles.quality(b"the grid reads back"), None);
    }

    #[test]
    fn a_negated_term_present_disqualifies_the_record() {
        let needles = Needles::build(&["scanner".to_owned()], &["offline".to_owned()]);
        assert_eq!(needles.quality(b"scanner offline now"), None);
    }

    #[test]
    fn a_negated_term_absent_does_not_affect_the_score() {
        let needles = Needles::build(&["scanner".to_owned()], &["offline".to_owned()]);
        assert!(needles.quality(b"scanner online now").is_some());
    }

    #[test]
    fn a_whole_word_match_scores_higher_than_a_mid_word_substring() {
        let needles = needles(&["scan"]);
        let whole = needles.quality(b"the scan finished").expect("a match");
        let partial = needles.quality(b"the scanner finished").expect("a match");
        assert!(whole > partial, "whole={whole} partial={partial}");
    }

    #[test]
    fn every_query_term_must_be_present() {
        let needles = needles(&["grid", "offline"]);
        assert_eq!(needles.quality(b"the grid is reading"), None);
        assert!(needles.quality(b"the grid is offline").is_some());
    }

    #[test]
    fn terms_found_close_together_score_higher_than_far_apart() {
        let needles = needles(&["grid", "scanner"]);
        let close = needles.quality(b"grid scanner reads back").expect("a match");
        let filler: String = std::iter::repeat_n('z', 500).collect();
        let far_text = format!("grid {filler} scanner reads back");
        let far = needles.quality(far_text.as_bytes()).expect("a match");
        assert!(close > far, "close={close} far={far}");
    }

    #[test]
    fn an_empty_needle_set_never_matches() {
        let needles = Needles::build(&[], &[]);
        assert_eq!(needles.quality(b"anything at all"), None);
        assert!(needles.is_empty());
    }
}
