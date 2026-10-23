//! What a flag says about itself.
//!
//! The rules are not re-checked here -- the server is the authority -- but
//! they are worth reading before a request rather than after one. So they
//! become the help text: "3 to 16 characters, a slug" above the flag,
//! instead of a 422 below it.

use zod::node::{Field, Node};
use zod::TextShape;

/// One field's help line.
///
/// The sentence the schema carries if it has one, because that sentence
/// was written for a person. Otherwise the label, otherwise nothing, and
/// then the rules in brackets after it.
pub fn sentence(field: &Field) -> String {
    let said = field.says.or(field.label).unwrap_or_default();
    match (said.is_empty(), rules(&field.node)) {
        (true, None) => String::new(),
        (true, Some(rules)) => rules,
        (false, None) => said.to_string(),
        (false, Some(rules)) => format!("{said} [{rules}]"),
    }
}

/// The rules, in the fewest words that are still true.
pub fn rules(node: &Node) -> Option<String> {
    let mut said: Vec<String> = Vec::new();
    match node.bare() {
        Node::Text(facts) => {
            said.extend(length(facts.min, facts.max, "character"));
            if let Some(shape) = shape(facts.shape) {
                said.push(shape.to_string());
            }
            if let Some(prefix) = facts.starts {
                said.push(format!("starts with `{prefix}`"));
            }
            if let Some(suffix) = facts.ends {
                said.push(format!("ends with `{suffix}`"));
            }
            if let Some(part) = facts.holds {
                said.push(format!("holds `{part}`"));
            }
        }
        Node::Int(facts) => said.extend(whole_span(facts)),
        Node::Real(facts) => said.extend(span(facts.min, facts.max)),
        Node::List { item, min, max } => {
            said.extend(length(*min, *max, "item"));
            if let Some(inner) = rules(item) {
                said.push(format!("each one {inner}"));
            }
        }
        _ => {}
    }
    (!said.is_empty()).then(|| said.join(", "))
}

/// A whole number's bounds, when they say more than its type already did.
///
/// Every node carries the range of the Rust type that holds it, so an
/// unbounded `i64` reads `-9223372036854775808 to 9223372036854775807`,
/// which tells a person nothing they wanted to know.
fn whole_span(facts: &zod::IntFacts) -> Option<String> {
    let low = (facts.min > i128::from(i64::MIN)).then_some(facts.min);
    let high = (facts.max < i128::from(i64::MAX)).then_some(facts.max);
    match (low, high) {
        (None, None) => None,
        (Some(min), None) => Some(format!("{min} or more")),
        (None, Some(max)) => Some(format!("{max} or less")),
        (Some(min), Some(max)) => Some(format!("{min} to {max}")),
    }
}

fn length(min: usize, max: usize, unit: &str) -> Option<String> {
    match (min, max) {
        (0, usize::MAX) => None,
        (a, b) if a == b => Some(format!("exactly {a} {unit}{}", plural(a))),
        (0, b) => Some(format!("at most {b} {unit}{}", plural(b))),
        (a, usize::MAX) => Some(format!("at least {a} {unit}{}", plural(a))),
        (a, b) => Some(format!("{a} to {b} {unit}s")),
    }
}

fn span(min: f64, max: f64) -> Option<String> {
    match (min.is_finite(), max.is_finite()) {
        (false, false) => None,
        (true, false) => Some(format!("{min} or more")),
        (false, true) => Some(format!("{max} or less")),
        (true, true) => Some(format!("{min} to {max}")),
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// A shape, named the way a person would say it.
const fn shape(shape: TextShape) -> Option<&'static str> {
    match shape {
        TextShape::Free => None,
        TextShape::Ascii => Some("ASCII only"),
        TextShape::Alnum => Some("letters and digits"),
        TextShape::Digits => Some("digits"),
        TextShape::Hex => Some("hexadecimal"),
        TextShape::Lower => Some("lowercase"),
        TextShape::Upper => Some("uppercase"),
        TextShape::Slug => Some("a slug"),
        TextShape::Email => Some("an email address"),
        TextShape::Uuid => Some("a UUID"),
        TextShape::DateIso => Some("a date, YYYY-MM-DD"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zod::{Entry, Int, Text};

    fn field_of<T: Entry>(at: usize) -> Field {
        T::describe().object().unwrap().fields[at].clone()
    }

    #[derive(Entry)]
    struct Shapes {
        #[text(min = 3, max = 16, slug)]
        #[says("Pick 3 to 16 lowercase letters, digits or hyphens")]
        handle: String,
        #[text(email)]
        #[label("Email address")]
        email: String,
        #[int(min = 13, max = 130)]
        age: i64,
        #[text(min = 2)]
        #[list(min = 1, max = 3)]
        tags: Vec<String>,
        plain: String,
        count: i64,
    }

    #[test]
    fn the_schemas_own_sentence_wins_and_the_rules_follow_it() {
        let said = sentence(&field_of::<Shapes>(0));
        assert_eq!(
            said,
            "Pick 3 to 16 lowercase letters, digits or hyphens [3 to 16 characters, a slug]"
        );
    }

    #[test]
    fn a_label_is_used_when_there_is_no_sentence() {
        assert_eq!(
            sentence(&field_of::<Shapes>(1)),
            "Email address [an email address]"
        );
    }

    #[test]
    fn a_field_with_neither_says_only_what_the_rules_say() {
        assert_eq!(sentence(&field_of::<Shapes>(2)), "13 to 130");
    }

    #[test]
    fn a_field_with_no_rules_at_all_says_nothing() {
        assert_eq!(sentence(&field_of::<Shapes>(4)), "");
        assert_eq!(
            sentence(&field_of::<Shapes>(5)),
            "",
            "an i64 bound is not a rule"
        );
    }

    #[test]
    fn a_list_says_how_many_and_what_each_one_is() {
        assert_eq!(
            sentence(&field_of::<Shapes>(3)),
            "1 to 3 items, each one at least 2 characters"
        );
    }

    #[test]
    fn one_is_singular() {
        use zod::Schema;
        assert_eq!(
            rules(&Text::new().min(1).node()).unwrap(),
            "at least 1 character"
        );
    }

    #[test]
    fn an_exact_length_says_exactly() {
        use zod::Schema;
        assert_eq!(
            rules(&Text::new().len(5).digits().node()).unwrap(),
            "exactly 5 characters, digits"
        );
    }

    #[test]
    fn a_one_sided_bound_says_one_side() {
        use zod::Schema;
        assert_eq!(rules(&Int::new().from(1).node()).unwrap(), "1 or more");
    }
}
