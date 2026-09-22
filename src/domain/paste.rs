//! Taking off the `<pasted_content>` wrapper Claude Code puts around text pasted into a prompt.
//!
//! The opening tag carries a short random id and the closing tag repeats it — `</pasted_content
//! id="7a1f">` rather than the well-formed `</pasted_content>` — so the close is matched on the
//! tag name and whatever follows it up to the `>`, not on an exact string.

use std::borrow::Cow;

const OPEN: &str = "<pasted_content";
const CLOSE: &str = "</pasted_content";

pub fn unwrapped(text: &str) -> Cow<'_, str> {
    let Some(first) = next(text) else { return Cow::Borrowed(text) };
    let mut out = String::with_capacity(text.len());
    let mut block = Some(first);
    let mut rest = text;
    while let Some(found) = block {
        out.push_str(found.before);
        out.push_str(found.body.trim_matches('\n'));
        rest = found.after;
        block = next(rest);
    }
    out.push_str(rest);
    Cow::Owned(out)
}

struct Block<'a> {
    before: &'a str,
    body: &'a str,
    after: &'a str,
}

fn next(text: &str) -> Option<Block<'_>> {
    let mut from = 0usize;
    loop {
        let rest = text.get(from..)?;
        let at = rest.find(OPEN)?;
        let open = from.checked_add(at)?;
        from = open.checked_add(OPEN.len())?;
        let Some(body) = opened(text, from) else { continue };
        let Some(close) = text.get(body..).and_then(|rest| rest.find(CLOSE)).and_then(|at| body.checked_add(at)) else {
            continue;
        };
        let Some(after) = text.get(close..).and_then(|rest| rest.find('>')).and_then(|at| close.checked_add(at)?.checked_add(1))
        else {
            continue;
        };
        return Some(Block { before: text.get(..open)?, body: text.get(body..close)?, after: text.get(after..)? });
    }
}

fn opened(text: &str, at: usize) -> Option<usize> {
    let rest = text.get(at..)?;
    if !rest.starts_with(['>', ' ', '\t']) {
        return None;
    }
    rest.find('>').and_then(|end| at.checked_add(end)?.checked_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_with_no_wrapper_in_it_is_borrowed_untouched() {
        let text = "read the grid scanner back to me";
        assert!(matches!(unwrapped(text), Cow::Borrowed(borrowed) if borrowed == text));
    }

    #[test]
    fn a_closing_tag_that_repeats_the_id_still_closes_the_block() {
        let text = "look at\n<pasted_content id=\"7a1f\">\nthe interlock falls back\n</pasted_content id=\"7a1f\">\nand say why";
        assert_eq!(unwrapped(text), "look at\nthe interlock falls back\nand say why");
    }

    #[test]
    fn a_well_formed_closing_tag_closes_the_block_too() {
        assert_eq!(unwrapped("<pasted_content>one</pasted_content>"), "one");
    }

    #[test]
    fn two_blocks_in_one_prompt_both_come_off() {
        let text =
            "a<pasted_content id=\"1\">one</pasted_content id=\"1\">b<pasted_content id=\"2\">two</pasted_content id=\"2\">c";
        assert_eq!(unwrapped(text), "aonebtwoc");
    }

    #[test]
    fn an_opening_tag_with_no_close_is_left_exactly_as_it_was_written() {
        let text = "what does <pasted_content id=\"7a1f\"> mean";
        assert_eq!(unwrapped(text), text);
    }

    #[test]
    fn a_tag_name_that_merely_starts_the_same_is_not_a_wrapper() {
        let text = "<pasted_contents>one</pasted_contents>";
        assert_eq!(unwrapped(text), text);
    }

    #[test]
    fn the_newlines_the_wrapper_adds_come_off_with_it() {
        let text = "before\n\n<pasted_content id=\"7a1f\">\n\nthe line\n\n</pasted_content id=\"7a1f\">\n\nafter";
        assert_eq!(unwrapped(text), "before\n\nthe line\n\nafter");
    }

    #[test]
    fn the_blank_line_a_paste_sits_alone_on_survives_so_it_stays_its_own_paragraph() {
        let text = "ship it\n\n<pasted_content id=\"7a1f\">\nthe interlock falls back\n</pasted_content id=\"7a1f\">\n\nunder the milestone";
        assert_eq!(unwrapped(text), "ship it\n\nthe interlock falls back\n\nunder the milestone");
    }

    #[test]
    fn an_empty_wrapper_leaves_the_prose_either_side_of_it() {
        let text = "before<pasted_content id=\"7a1f\">\n</pasted_content id=\"7a1f\">after";
        assert_eq!(unwrapped(text), "beforeafter");
    }
}
