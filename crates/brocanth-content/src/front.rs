//! What every collection's front matter type gives, and a file as the
//! binary embeds it.

use serde::de::DeserializeOwned;

use crate::Date;

/// What a collection reads from each entry's front matter.
///
/// The project's type implements it: the page, the feed and the drafts
/// rule read an entry through these, and the type keeps any other field
/// for its own templates. Only `title` is required; the rest default to
/// nothing.
pub trait FrontMatter: DeserializeOwned {
    /// The entry's title. Empty is refused at build time.
    fn title(&self) -> &str;

    /// The day it was published. The list is newest first; an entry with
    /// no date sorts last and is left out of the feed, which needs one.
    fn date(&self) -> Option<Date> {
        None
    }

    /// The day it last changed, when that is not `date`.
    fn updated(&self) -> Option<Date> {
        None
    }

    /// One or two sentences: the list's summary, the page's description,
    /// the feed's summary.
    fn description(&self) -> Option<&str> {
        None
    }

    /// Its tags, as written.
    fn tags(&self) -> &[String] {
        &[]
    }

    /// Whether it is a draft: listed only where drafts are shown, and never
    /// in a feed or a static export.
    fn draft(&self) -> bool {
        false
    }
}

/// A file of a collection, as `build.rs` writes it into the binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source {
    /// The file, from the project root: `content/posts/hello.md`.
    pub path: &'static str,
    /// The file's name without `.md` and without a locale:
    /// `hello` for `hello.md` and for `hello.fr.md`.
    pub slug: &'static str,
    /// The locale a translation names: `fr` for `hello.fr.md`. `None` for
    /// the file in the default language.
    pub locale: Option<&'static str>,
    /// The file's text.
    pub text: &'static str,
}

/// A file name read as `(slug, locale)`: `hello.md` is `("hello", None)`,
/// `hello.fr.md` is `("hello", Some("fr"))`.
///
/// # Errors
///
/// A name that is not a slug, with a `help:` line. A slug is the URL
/// segment, so it is lower case letters, digits and single `-`.
pub fn read_name(name: &str) -> Result<(&str, Option<&str>), String> {
    let stem = name
        .strip_suffix(".md")
        .ok_or_else(|| format!("`{name}` is not a Markdown file\nhelp: name it `<slug>.md`"))?;
    let (slug, locale) = match stem.split_once('.') {
        Some((slug, locale)) => (slug, Some(locale)),
        None => (stem, None),
    };
    let slug_ok = !slug.is_empty()
        && slug.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        });
    if !slug_ok {
        return Err(format!(
            "`{slug}` cannot be a URL segment\n\
             help: rename the file with lower case letters, digits and single `-`: `my-first-post.md`"
        ));
    }
    if let Some(locale) = locale {
        let mut parts = locale.split('-');
        let language = parts.next().unwrap_or("");
        let locale_ok = (2..=3).contains(&language.len())
            && language.chars().all(|c| c.is_ascii_lowercase())
            && parts.all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()));
        if !locale_ok {
            return Err(format!(
                "`{locale}` is not a locale\n\
                 help: name a translation `<slug>.<locale>.md`, such as `{slug}.fr.md`"
            ));
        }
    }
    Ok((slug, locale))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_a_slug_and_maybe_a_locale() {
        assert_eq!(read_name("hello.md"), Ok(("hello", None)));
        assert_eq!(read_name("hello-2.fr.md"), Ok(("hello-2", Some("fr"))));
        assert_eq!(read_name("hello.pt-BR.md"), Ok(("hello", Some("pt-BR"))));
        for bad in [
            "Hello.md", "a--b.md", "-a.md", "a_b.md", "a.md.txt", "a.FR.md", "a.f.md",
        ] {
            let e = read_name(bad).unwrap_err();
            assert!(e.contains("\nhelp: "), "{bad}: {e}");
        }
    }
}
