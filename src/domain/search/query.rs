//! The query syntax: bare terms AND together, `"quoted phrases"`, `is:user|assistant|tool|
//! thinking|any`, `project:name`, `-negation`. Anything unrecognized is a literal term.
//!
//! A query that names no category searches `Category::Said` — what a human or the assistant
//! wrote. `Said` is the default rather than a token, so it is not parsed and not in the help
//! string; naming any other category is how the rest of the corpus is reached.
//!
//! Both bare terms and phrases end up as literal byte strings to search for — a phrase is
//! only a term that happened to contain spaces — so `Query` does not keep them apart.

use crate::domain::cache::shard::Field;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Said,
    User,
    Assistant,
    Tool,
    Thinking,
    Any,
}

impl Category {
    pub const fn matches(self, field: Field) -> bool {
        match self {
            Self::Said => matches!(field, Field::UserPrompt | Field::AssistantText),
            Self::User => matches!(field, Field::UserPrompt),
            Self::Assistant => matches!(field, Field::AssistantText),
            Self::Tool => matches!(field, Field::ToolInput | Field::ToolResult),
            Self::Thinking => matches!(field, Field::Thinking),
            Self::Any => true,
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "tool" => Some(Self::Tool),
            "thinking" => Some(Self::Thinking),
            "any" => Some(Self::Any),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub positive: Vec<String>,
    pub negative: Vec<String>,
    pub category: Option<Category>,
    pub project: Option<String>,
}

impl Query {
    pub const fn is_empty(&self) -> bool {
        self.positive.is_empty()
    }
}

pub fn parse(input: &str) -> Query {
    let mut query = Query::default();
    for token in tokenize(input) {
        classify(&token, &mut query);
    }
    query
}

fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;

    for character in input.chars() {
        if character == '"' {
            quoted = !quoted;
            current.push(character);
            continue;
        }
        if character.is_whitespace() && !quoted {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(character);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn classify(token: &str, query: &mut Query) {
    let (negated, rest) = match token.strip_prefix('-') {
        Some(after) if !after.is_empty() => (true, after),
        _ => (false, token),
    };

    if let Some(phrase) = rest.strip_prefix('"') {
        let phrase = phrase.strip_suffix('"').unwrap_or(phrase).trim();
        if !phrase.is_empty() {
            push(query, negated, phrase);
        }
        return;
    }

    if !negated {
        if let Some(value) = rest.strip_prefix("is:")
            && let Some(category) = Category::parse(value)
        {
            query.category = Some(category);
            return;
        }
        if let Some(value) = rest.strip_prefix("project:")
            && !value.is_empty()
        {
            query.project = Some(value.to_lowercase());
            return;
        }
    }

    if !rest.is_empty() {
        push(query, negated, rest);
    }
}

fn push(query: &mut Query, negated: bool, text: &str) {
    let lowered = text.to_lowercase();
    if negated { query.negative.push(lowered) } else { query.positive.push(lowered) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_terms_become_positive_lowercase_literals() {
        let query = parse("Grid Scanner");
        assert_eq!(query.positive, ["grid", "scanner"]);
    }

    #[test]
    fn a_quoted_phrase_stays_one_literal() {
        let query = parse(r#"read "the grid scanner" back"#);
        assert_eq!(query.positive, ["read", "the grid scanner", "back"]);
    }

    #[test]
    fn a_dash_prefixed_term_is_negated() {
        let query = parse("scanner -offline");
        assert_eq!(query.positive, ["scanner"]);
        assert_eq!(query.negative, ["offline"]);
    }

    #[test]
    fn a_dash_prefixed_phrase_is_negated() {
        let query = parse(r#"scanner -"power off""#);
        assert_eq!(query.positive, ["scanner"]);
        assert_eq!(query.negative, ["power off"]);
    }

    #[test]
    fn is_filters_parse_the_four_known_categories() {
        assert_eq!(parse("is:user").category, Some(Category::User));
        assert_eq!(parse("is:assistant").category, Some(Category::Assistant));
        assert_eq!(parse("is:tool").category, Some(Category::Tool));
        assert_eq!(parse("is:thinking").category, Some(Category::Thinking));
    }

    #[test]
    fn an_unknown_is_value_is_a_literal_term_not_a_filter() {
        let query = parse("is:robot");
        assert_eq!(query.category, None);
        assert_eq!(query.positive, ["is:robot"]);
    }

    #[test]
    fn project_filters_lowercase_their_value() {
        let query = parse("project:Vade scanner");
        assert_eq!(query.project.as_deref(), Some("vade"));
        assert_eq!(query.positive, ["scanner"]);
    }

    #[test]
    fn a_bare_dash_is_literal_and_a_double_dash_negates_a_single_dash() {
        let query = parse("- -- foo");
        assert_eq!(query.positive, ["-", "foo"]);
        assert_eq!(query.negative, ["-"]);
    }

    #[test]
    fn an_unterminated_quote_still_reads_to_the_end() {
        let query = parse(r#""read the grid"#);
        assert_eq!(query.positive, ["read the grid"]);
    }

    #[test]
    fn whitespace_only_input_is_an_empty_query() {
        let query = parse("   ");
        assert!(query.is_empty());
        assert_eq!(query, Query::default());
    }

    #[test]
    fn category_matching_maps_tool_to_both_tool_fields() {
        assert!(Category::Tool.matches(Field::ToolInput));
        assert!(Category::Tool.matches(Field::ToolResult));
        assert!(!Category::Tool.matches(Field::UserPrompt));
        assert!(Category::User.matches(Field::UserPrompt));
        assert!(!Category::User.matches(Field::AssistantText));
    }

    #[test]
    fn the_said_category_takes_user_and_assistant_text_and_nothing_else() {
        assert!(Category::Said.matches(Field::UserPrompt));
        assert!(Category::Said.matches(Field::AssistantText));
        assert!(!Category::Said.matches(Field::Thinking));
        assert!(!Category::Said.matches(Field::ToolInput));
        assert!(!Category::Said.matches(Field::ToolResult));
    }

    #[test]
    fn the_any_category_takes_every_field() {
        for field in [Field::UserPrompt, Field::AssistantText, Field::Thinking, Field::ToolInput, Field::ToolResult] {
            assert!(Category::Any.matches(field));
        }
    }

    #[test]
    fn is_any_parses_as_a_category_rather_than_a_literal_term() {
        let query = parse("is:any grid");
        assert_eq!(query.category, Some(Category::Any));
        assert_eq!(query.positive, ["grid"]);
    }

    #[test]
    fn said_is_the_default_and_is_not_reachable_as_a_token() {
        let query = parse("is:said grid");
        assert_eq!(query.category, None);
        assert_eq!(query.positive, ["is:said", "grid"]);
    }
}
