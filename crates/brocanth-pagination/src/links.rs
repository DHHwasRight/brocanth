//! What a request asked for, and the links a page answers with.
//!
//! A link to page 3 must keep everything else the request carried: the
//! search, the filters, the sort. [`Links`] holds the query string without
//! `page`, `after` and `before`, and writes each link from it, so a filter set on page
//! one is still set on page three.

use std::fmt;

use crate::cursor::{Cursor, CursorError, KeysetPage};
use crate::page::Page;
use crate::query;

/// The query-string keys this crate reads and writes.
pub const PAGE: &str = "page";
/// The page size.
pub const PER_PAGE: &str = "per_page";
/// The keyset cursor of a next page.
pub const AFTER: &str = "after";
/// The keyset cursor of a previous page.
pub const BEFORE: &str = "before";

/// What a request asked for: `page`, `per_page`, and `after` or `before`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Params {
    /// The page, counting from one. One when absent.
    pub page: u64,
    /// The page size, or `None` for the list's default.
    pub per_page: Option<u64>,
    /// The keyset cursor of a next page: read the rows after it.
    pub after: Option<Cursor>,
    /// The keyset cursor of a previous page: read the rows before it. Never
    /// set together with `after`.
    pub before: Option<Cursor>,
}

/// A value in the query string that is not what its key needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamError {
    /// `page` or `per_page` is not a whole number.
    NotANumber {
        /// The key.
        key: &'static str,
        /// What it held.
        value: String,
    },
    /// `after` or `before` is not a cursor.
    Cursor(CursorError),
    /// `after` and `before` are both set: a page cannot go both ways.
    BothCursors,
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParamError::NotANumber { key, value } => write!(
                f,
                "`{key}` is not a whole number: `{value}`\n\
                 help: write `{key}=2`, or drop `{key}` to get the default"
            ),
            ParamError::Cursor(error) => error.fmt(f),
            ParamError::BothCursors => f.write_str(
                "`after` and `before` are both set\n\
                 help: follow the `next` or the `previous` link of a page; each sets one of them",
            ),
        }
    }
}

impl std::error::Error for ParamError {}

impl Params {
    /// Reads `page`, `per_page`, `after` and `before` from a query string.
    ///
    /// A number that is not a number is an error, not page one: serving page
    /// one to somebody who asked for page `two` is the quiet repair that makes
    /// a paginator untrustworthy. A page below one is one.
    ///
    /// # Errors
    ///
    /// [`ParamError`], with a `help:` line, for a bad value.
    pub fn parse(query_string: &str) -> Result<Params, ParamError> {
        let number = |key: &'static str| -> Result<Option<u64>, ParamError> {
            match query::param(query_string, key) {
                None => Ok(None),
                Some(value) if value.is_empty() => Ok(None),
                Some(value) => value
                    .parse()
                    .map(Some)
                    .map_err(|_| ParamError::NotANumber { key, value }),
            }
        };
        let cursor = |key: &str| match query::param(query_string, key) {
            Some(token) if !token.is_empty() => {
                Cursor::parse(&token).map(Some).map_err(ParamError::Cursor)
            }
            _ => Ok(None),
        };
        let after = cursor(AFTER)?;
        let before = cursor(BEFORE)?;
        if after.is_some() && before.is_some() {
            return Err(ParamError::BothCursors);
        }
        Ok(Params {
            page: number(PAGE)?.unwrap_or(1).max(1),
            per_page: number(PER_PAGE)?,
            after,
            before,
        })
    }
}

/// The base every link of one list is written from: the path, and the query
/// string without `page`, `after` and `before`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Links {
    path: String,
    kept: Vec<(String, String)>,
}

impl Links {
    /// The links of the list at `path`, keeping every key of `query_string`
    /// but `page`, `after` and `before`.
    pub fn new(path: &str, query_string: &str) -> Links {
        Links {
            path: path.to_string(),
            kept: query::pairs(query_string)
                .into_iter()
                .filter(|(key, _)| key != PAGE && key != AFTER && key != BEFORE)
                .collect(),
        }
    }

    fn with(&self, extra: Option<(&str, &str)>) -> String {
        let pairs = self
            .kept
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .chain(extra);
        let query = query::join(pairs);
        if query.is_empty() {
            self.path.clone()
        } else {
            format!("{}?{query}", self.path)
        }
    }

    /// The first page: the path and the kept keys, with no `page`.
    pub fn first(&self) -> String {
        self.with(None)
    }

    /// Page `number`. Page one is [`Links::first`], so the first page has one
    /// address.
    pub fn page(&self, number: u64) -> String {
        if number <= 1 {
            self.first()
        } else {
            self.with(Some((PAGE, &number.to_string())))
        }
    }

    /// The keyset page that starts after `cursor`.
    pub fn after(&self, cursor: &Cursor) -> String {
        self.with(Some((AFTER, &cursor.token())))
    }

    /// The keyset page that ends before `cursor`.
    pub fn before(&self, cursor: &Cursor) -> String {
        self.with(Some((BEFORE, &cursor.token())))
    }
}

/// One numbered link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageLink {
    /// The page number.
    pub number: u64,
    /// Where it goes.
    pub href: String,
    /// Whether it is the page being shown.
    pub current: bool,
}

/// Everything a template needs to draw an offset pager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pager {
    /// Where the page sits in the whole.
    pub page: Page,
    /// The numbered links, centred on the current page.
    pub pages: Vec<PageLink>,
    /// The previous page, when there is one.
    pub previous: Option<String>,
    /// The next page, when there is one.
    pub next: Option<String>,
}

impl Pager {
    /// How many numbered links a pager shows when nobody says.
    pub const SPAN: u64 = 7;

    /// A pager with links written by `href`. Use this when the list writes
    /// its own query string, as the admin does.
    pub fn with(page: Page, span: u64, href: impl Fn(u64) -> String) -> Pager {
        let pages = page
            .window(span)
            .into_iter()
            .map(|number| PageLink {
                href: href(number),
                current: number == page.number(),
                number,
            })
            .collect();
        Pager {
            pages,
            previous: page.previous().map(&href),
            next: page.next().map(&href),
            page,
        }
    }

    /// A pager over `links`, with [`Pager::SPAN`] numbered links.
    pub fn new(page: Page, links: &Links) -> Pager {
        Pager::with(page, Pager::SPAN, |n| links.page(n))
    }

    /// `1–25 of 95 · page 1 of 4`. See [`Page::summary`].
    pub fn summary(&self) -> String {
        self.page.summary()
    }

    /// Whether there is more than one page, so a pager is worth drawing.
    pub fn is_needed(&self) -> bool {
        self.page.total_pages() > 1
    }
}

/// Everything a template needs to draw a keyset pager: the first page, the
/// previous one and the next one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeysetPager {
    /// The first page, when this is not it.
    pub first: Option<String>,
    /// The previous page, when there is one.
    pub previous: Option<String>,
    /// The next page, when there is one.
    pub next: Option<String>,
}

impl KeysetPager {
    /// The links of a keyset page. `read_with` is the cursor the page was
    /// read with (`after` or `before`), `None` on the first page.
    pub fn new<R>(links: &Links, read_with: Option<&Cursor>, page: &KeysetPage<R>) -> KeysetPager {
        KeysetPager {
            first: read_with.map(|_| links.first()),
            previous: page.previous.as_ref().map(|cursor| links.before(cursor)),
            next: page.next.as_ref().map(|cursor| links.after(cursor)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cursor::Key;

    #[test]
    fn a_link_keeps_the_filters_and_drops_the_old_page() {
        let links = Links::new("/posts", "status=published&page=3&q=red+shoes&after=i9");
        assert_eq!(
            links.page(4),
            "/posts?status=published&q=red%20shoes&page=4"
        );
        assert_eq!(links.page(1), "/posts?status=published&q=red%20shoes");
        assert_eq!(
            links.after(&Cursor::new([Key::Int(7)])),
            "/posts?status=published&q=red%20shoes&after=i7"
        );
        assert_eq!(Links::new("/posts", "").page(1), "/posts");
    }

    #[test]
    fn a_bad_number_is_an_error_and_not_page_one() {
        let error = Params::parse("page=two").unwrap_err();
        assert!(error.to_string().contains("`page` is not a whole number"));
        assert!(error.to_string().contains("help:"));
        assert!(Params::parse("after=nope").is_err());
        assert_eq!(Params::parse("page=0").unwrap().page, 1);
        assert_eq!(Params::parse("").unwrap().page, 1);
        let params = Params::parse("page=3&per_page=10&after=i5").unwrap();
        assert_eq!(params.per_page, Some(10));
        assert_eq!(params.after, Some(Cursor::new([Key::Int(5)])));
    }

    #[test]
    fn a_pager_marks_the_current_page_and_its_neighbours() {
        let pager = Pager::new(Page::new(2, 10, 35), &Links::new("/p", "q=x"));
        assert_eq!(pager.previous.as_deref(), Some("/p?q=x"));
        assert_eq!(pager.next.as_deref(), Some("/p?q=x&page=3"));
        assert_eq!(
            pager.pages.iter().map(|l| l.number).collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert!(pager.pages[1].current);
        assert!(pager.is_needed());
        assert_eq!(pager.summary(), "11–20 of 35 · page 2 of 4");
    }

    #[test]
    fn a_keyset_pager_links_both_ways_and_home() {
        let key = |n: &i64| Cursor::new([Key::Int(*n)]);
        let links = Links::new("/feed", "after=i3&before=i1");
        let first = KeysetPager::new(&links, None, &KeysetPage::from_rows(vec![1, 2, 3], 2, key));
        assert_eq!(first.first, None);
        assert_eq!(first.previous, None);
        assert_eq!(first.next.as_deref(), Some("/feed?after=i2"));
        let after = Cursor::new([Key::Int(2)]);
        let last = KeysetPager::new(&links, Some(&after), &KeysetPage::after(vec![3], 2, key));
        assert_eq!(last.first.as_deref(), Some("/feed"));
        assert_eq!(last.previous.as_deref(), Some("/feed?before=i3"));
        assert_eq!(last.next, None);
    }

    #[test]
    fn after_and_before_together_are_an_error() {
        let error = Params::parse("after=i1&before=i2").unwrap_err();
        assert_eq!(error, ParamError::BothCursors);
        assert!(error.to_string().contains("help:"));
        let params = Params::parse("before=i2").unwrap();
        assert_eq!(params.before, Some(Cursor::new([Key::Int(2)])));
        assert!(Params::parse("before=nope").is_err());
    }
}
