#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! # Decisions
//!
//! - **An inbox holds one account.** [`NotificationStore::inbox`] binds the
//!   account once; [`Inbox`] has no method that names another. The account
//!   is in the `WHERE` of every statement, beside the row's id, so a forged
//!   id from another account matches nothing. A policy `Condition` can only
//!   narrow it further.
//! - **The account is text.** Whatever `brocanth-auth` or the application
//!   uses as an id (a UUID, a serial, an email) is stored as its text, so
//!   the table needs no foreign key into a table it does not own.
//! - **Keyset pages, newest first.** `list(before, limit)` reads the rows
//!   below an id. A new notification while a reader pages does not shift
//!   the next page, as an offset would.
//! - **A link is a path or http(s).** [`Notice::url`] refuses anything
//!   else: the page writes it into an `href`, and a `javascript:` URL there
//!   runs in the reader's browser.
//! - **Mail and live push belong to the caller.** The store writes a row.
//!   The generated `notify` helper sends the mail (with `mail`) and the
//!   realtime change (with `realtime`), because only the project knows the
//!   reader's address and its broker.

#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
mod plugin;
#[cfg(feature = "runtime")]
mod store;

#[cfg(feature = "runtime")]
pub use plugin::{NotificationsConfig, NotificationsPlugin};
#[cfg(feature = "runtime")]
pub use store::{Inbox, Item, NotificationStore, NotifyError, SCHEMA, TABLE};

/// The rule algebra the inbox's `list` takes a condition from, so a page
/// writes a rule without a dependency of its own.
#[cfg(feature = "runtime")]
pub use brocanth_policy as policy;

/// Publishes a stored notification as a `created` change on
/// [`TABLE`], with its `id`, `account`, `kind` and `title` columns, so a
/// realtime subscription whose rule is `account = <the reader>` receives it.
#[cfg(feature = "realtime")]
pub fn publish(
    broker: &brocanth_realtime::Broker,
    account: &str,
    id: i64,
    notice: &Notice,
) -> brocanth_realtime::Delivery {
    let row = brocanth_realtime::Row::new()
        .with("id", id)
        .with("account", account.to_string())
        .with("kind", notice.kind.clone())
        .with("title", notice.title.clone());
    broker.publish_with_row(&brocanth_events::RowChange::created(TABLE, id), &row)
}

/// What to tell an account: a kind, a title, a body and a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// What kind of event: `comment`, `invoice`. For a filter or an icon.
    pub kind: String,
    /// The line the inbox shows.
    pub title: String,
    /// More text, empty for none.
    pub body: String,
    /// Where the notification leads: a path on this site or an http(s) URL.
    pub url: Option<String>,
}

impl Notice {
    /// A notice of `kind`, titled `title`.
    pub fn new(kind: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            title: title.into(),
            body: String::new(),
            url: None,
        }
    }

    /// The text under the title.
    #[must_use]
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    /// Where the notification leads. [`NotificationStore::notify`] refuses
    /// a link that is not a path on this site or an http(s) URL.
    #[must_use]
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
}

/// Whether `url` may go into an `href`: a path on this site (`/posts/7`,
/// not `//evil.example`), or an `http`/`https` URL. No control character.
pub fn safe_url(url: &str) -> bool {
    if url.chars().any(char::is_control) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    (url.starts_with('/') && !url.starts_with("//") && !url.starts_with("/\\"))
        || lower.starts_with("https://")
        || lower.starts_with("http://")
}

/// What a mark-read form asks for, read from its urlencoded body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkRead {
    /// `id=<n>`: one notification.
    One(i64),
    /// `all=1`: every unread notification of the account.
    All,
}

impl MarkRead {
    /// Reads `id=<n>` or `all=1` from a form body. `None` for anything
    /// else, so a page answers 400.
    pub fn from_form(body: &[u8]) -> Option<MarkRead> {
        let text = std::str::from_utf8(body).ok()?;
        let mut found = None;
        for pair in text.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match key {
                "all" if value == "1" => return Some(MarkRead::All),
                "id" => found = value.parse::<i64>().ok().map(MarkRead::One),
                _ => {}
            }
        }
        found
    }
}

/// Text into HTML, for the five characters that matter in an element and an
/// attribute. For the generated page, which writes HTML by hand.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// The columns of the table, for a policy rule over the inbox.
#[cfg(feature = "runtime")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NotificationColumn {
    /// `bigserial`, the primary key.
    Id,
    /// The account the notification belongs to, as text.
    Account,
    /// The kind.
    Kind,
    /// The title.
    Title,
}

#[cfg(feature = "runtime")]
impl brocanth_policy::Column for NotificationColumn {
    const ALL: &'static [Self] = &[Self::Id, Self::Account, Self::Kind, Self::Title];

    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Account => "account",
            Self::Kind => "kind",
            Self::Title => "title",
        }
    }
}

/// The entity a policy over the inbox is written for.
#[cfg(feature = "runtime")]
pub struct Notification;

#[cfg(feature = "runtime")]
impl brocanth_policy::Entity for Notification {
    type Column = NotificationColumn;
    const TABLE: brocanth_policy::Ident = brocanth_policy::Ident::must("brocanth_notifications");
    const PRIMARY_KEY: Self::Column = NotificationColumn::Id;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_is_a_local_path_or_http() {
        for good in [
            "/posts/7",
            "/",
            "https://example.com/a",
            "HTTP://example.com",
        ] {
            assert!(safe_url(good), "{good}");
        }
        for bad in [
            "javascript:alert(1)",
            "//evil.example",
            "/\\evil.example",
            "data:text/html,x",
            "posts/7",
            "/a\nb",
        ] {
            assert!(!safe_url(bad), "{bad}");
        }
    }

    #[test]
    fn a_mark_read_form_names_one_id_or_all() {
        assert_eq!(MarkRead::from_form(b"id=42"), Some(MarkRead::One(42)));
        assert_eq!(
            MarkRead::from_form(b"_csrf_token=x&id=7"),
            Some(MarkRead::One(7))
        );
        assert_eq!(MarkRead::from_form(b"all=1&id=7"), Some(MarkRead::All));
        assert_eq!(MarkRead::from_form(b"id=abc"), None);
        assert_eq!(MarkRead::from_form(b"all=yes"), None);
        assert_eq!(MarkRead::from_form(b""), None);
    }

    #[test]
    fn escape_covers_markup_and_quotes() {
        assert_eq!(
            escape("<a href=\"x\">'&"),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;"
        );
    }
}
