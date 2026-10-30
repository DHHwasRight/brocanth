//! The 422 path for a form: the same form again, with its errors.
//!
//! [`Answer`](crate::Answer) is the refusal for a caller that reads JSON.
//! A browser, or dp-swap's `dp-post`, wants HTML: the form it sent, its
//! values kept, and a message next to each field that was wrong. Status
//! 422, so dp-swap swaps it in where the form was and a plain browser
//! shows it as the page.
//!
//! ```
//! use brocanth_input::{rerender, Entrance, FieldErrors};
//! use zod::{Entry, Text};
//!
//! #[derive(Entry)]
//! struct Signup {
//!     #[check(Text::new().min(3))]
//!     handle: String,
//! }
//!
//! fn form(errors: &FieldErrors) -> String {
//!     format!(
//!         r#"<form><input name="handle">{}</form>"#,
//!         errors.render("handle")
//!     )
//! }
//!
//! let at = Entrance::new(Some("application/x-www-form-urlencoded"), "", b"handle=ab");
//! let faults = Signup::parse(at.input().unwrap()).err().unwrap();
//! let sent = rerender(&faults, form);
//! assert_eq!(sent.status, 422);
//! ```

use zod::Faults;

/// The messages for each field, read off [`Faults`].
///
/// Keyed by the field's path with dots, `address.city`, which is the
/// name a nested form field carries. A fault at the root of the value is
/// under `""`: it belongs to the form, not to a field.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldErrors(Vec<(String, Vec<String>)>);

impl FieldErrors {
    /// No errors, for the first render of the form.
    pub const fn none() -> FieldErrors {
        FieldErrors(Vec::new())
    }

    /// Every fault, grouped by field.
    pub fn from_faults(faults: &Faults) -> FieldErrors {
        FieldErrors(faults.by_path())
    }

    /// Whether the form passed.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The messages for one field, in the order the schema found them.
    pub fn of(&self, field: &str) -> &[String] {
        self.0
            .iter()
            .find(|(name, _)| name == field)
            .map_or(&[], |(_, messages)| messages.as_slice())
    }

    /// Whether one field is wrong. For an `aria-invalid` attribute.
    pub fn has(&self, field: &str) -> bool {
        !self.of(field).is_empty()
    }

    /// Every field and its messages.
    pub fn all(&self) -> &[(String, Vec<String>)] {
        &self.0
    }

    /// The messages for one field, as markup to put beside its input.
    ///
    /// Empty when the field is fine, so a template writes it
    /// unconditionally. One `<p>` per message, escaped, with the id
    /// `{field}-error` for the input's `aria-describedby`.
    pub fn render(&self, field: &str) -> String {
        let messages = self.of(field);
        if messages.is_empty() {
            return String::new();
        }
        let id = escape(&format!("{field}-error"));
        let mut out = format!(r#"<div class="field-error" id="{id}" role="alert">"#);
        for message in messages {
            out.push_str("<p>");
            out.push_str(&escape(message));
            out.push_str("</p>");
        }
        out.push_str("</div>");
        out
    }
}

/// The form again, with its errors, as a 422.
///
/// `render` is the template that drew the form the first time, given the
/// errors this time. It is the same function both times, so the two
/// renders cannot drift.
pub fn rerender(
    faults: &Faults,
    render: impl FnOnce(&FieldErrors) -> String,
) -> brocanth_server::Response {
    brocanth_server::Response::html(422, render(&FieldErrors::from_faults(faults)))
}

fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Entrance;
    use zod::{Entry, Int, Text};

    #[derive(Entry, Debug)]
    #[allow(dead_code)]
    struct Signup {
        #[check(Text::new().min(3).max(16).slug())]
        handle: String,
        #[check(Int::new().from(13).to(130))]
        age: i64,
    }

    fn faults(body: &[u8]) -> Faults {
        let at = Entrance::new(Some("application/x-www-form-urlencoded"), "", body);
        Signup::parse(at.input().expect("a form")).expect_err("faults")
    }

    #[test]
    fn each_field_gets_its_own_messages() {
        let errors = FieldErrors::from_faults(&faults(b"handle=ab&age=36"));
        assert!(errors.has("handle"));
        assert!(!errors.has("age"));
        assert_eq!(errors.render("age"), "");
        let html = errors.render("handle");
        assert!(html.starts_with(r#"<div class="field-error" id="handle-error" role="alert">"#));
    }

    #[test]
    fn the_rerender_is_a_422_with_the_form_in_it() {
        let sent = rerender(&faults(b"handle=ab&age=7"), |errors| {
            format!(
                "<form>{}{}</form>",
                errors.render("handle"),
                errors.render("age")
            )
        });
        assert_eq!(sent.status, 422);
        assert_eq!(
            sent.header("content-type"),
            Some("text/html; charset=utf-8")
        );
    }

    #[test]
    fn a_message_is_escaped() {
        let errors = FieldErrors(vec![("x".into(), vec!["<b>&".into()])]);
        assert!(errors.render("x").contains("<p>&lt;b&gt;&amp;</p>"));
    }
}
