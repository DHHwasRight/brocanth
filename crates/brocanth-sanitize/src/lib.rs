#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! ---
//!
//! # The two types
//!
//! [`Policy`] is an allow-list: the tags, the attributes and the URL
//! schemes that may stay. Everything else goes. [`SafeHtml`] is what a
//! policy returns, and the only way to get one from a string a user wrote.
//!
//! A template writes a `SafeHtml` as markup: `{! body }` in rsx, or as a
//! child of a component with the `rsx` feature. A plain `String` in the
//! same place is a choice somebody can see in review. In `{ body }` the
//! value is escaped, which shows the tags as text and runs nothing: the
//! wrong place fails safe.
//!
//! # Why ammonia, and not a sanitizer written here
//!
//! A sanitizer that reads HTML with its own rules is wrong wherever the
//! browser reads it with different ones. That gap is where mutation XSS
//! lives: `<noscript>`, `<svg>` and `<math>` change how the next tags parse,
//! and a regex or a small tokenizer does not know it. `ammonia` parses with
//! `html5ever`, which implements the WHATWG parsing algorithm the browsers
//! implement, keeps only the allowed nodes, and serialises the tree again.
//! So the output is markup the browser parses into the tree that was
//! checked. It is the sanitizer behind `nh3`, which Django's bleach
//! successor uses. Writing one here would mean writing an HTML5 parser, and
//! the cost of a bug is a stored XSS in every page that shows a comment.
//!
//! The price is the parser: `html5ever` and its string tables. That is why
//! this is a crate of its own and not a module of `brocanth-pages`.

#[cfg(feature = "runtime")]
mod policy;

#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
pub use policy::{comment, rich_text, text, Policy};

/// Re-exported, so [`Policy::from_builder`] needs no second dependency line.
#[cfg(feature = "runtime")]
pub use ammonia;

/// HTML that a [`Policy`] cleaned, or text that was escaped.
///
/// The field is private, so a value of this type came from
/// [`Policy::clean`], [`SafeHtml::escape`] or [`SafeHtml::from_static`].
/// There is no `From<String>` and no `Deserialize`: either would make a
/// `SafeHtml` out of anything.
///
/// Store the source the user wrote, and clean it when the page renders.
/// A policy that gets stricter then applies to old rows too.
#[cfg(feature = "runtime")]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct SafeHtml(String);

#[cfg(feature = "runtime")]
impl SafeHtml {
    /// Plain text as HTML: `<`, `>`, `&`, `"` and `'` become entities.
    pub fn escape(text: &str) -> Self {
        let mut out = String::with_capacity(text.len());
        for ch in text.chars() {
            match ch {
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '&' => out.push_str("&amp;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                _ => out.push(ch),
            }
        }
        Self(out)
    }

    /// Markup written in the source of this program, trusted as it is.
    ///
    /// `&'static str` only, so a value read at run time cannot pass: a
    /// string from a request or a row is never `'static` without a leak
    /// somebody wrote on purpose.
    pub fn from_static(html: &'static str) -> Self {
        Self(html.to_string())
    }

    /// The markup.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The markup, by value.
    pub fn into_string(self) -> String {
        self.0
    }

    /// Whether there is no markup at all.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Joins two values. Both are safe, so the result is.
    #[must_use]
    pub fn concat(mut self, other: &SafeHtml) -> Self {
        self.0.push_str(&other.0);
        self
    }

    pub(crate) fn cleaned(html: String) -> Self {
        Self(html)
    }
}

#[cfg(feature = "runtime")]
impl std::fmt::Display for SafeHtml {
    /// The markup as it is. Escaped again by `{ value }` in a template;
    /// written as markup by `{! value }`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(feature = "runtime")]
impl AsRef<str> for SafeHtml {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A `SafeHtml` is a child a component writes as markup.
#[cfg(feature = "rsx")]
impl rsx::Template for SafeHtml {
    fn render(self, w: &mut rsx::Writer<'_>) -> rsx::Result {
        w.raw(&self.0)
    }
}

/// A borrowed `SafeHtml` too, so a row can be rendered without a clone.
#[cfg(feature = "rsx")]
impl rsx::Template for &SafeHtml {
    fn render(self, w: &mut rsx::Writer<'_>) -> rsx::Result {
        w.raw(&self.0)
    }
}
