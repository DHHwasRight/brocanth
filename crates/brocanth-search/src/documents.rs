//! The site index: one table of everything a visitor can find.
//!
//! pg_search calls it multisearch, django-watson calls it a search entry. A
//! post, a page and a product each write a row here when they change: a
//! kind, a key, a title, a body and the URL of the thing. One query then
//! searches them all, and the search page needs no join.
//!
//! The row is a copy, so it can go stale. Write it in the same transaction
//! as the thing it describes (pass the `Tx` to `upsert`), or from a job,
//! and delete it with the thing.
//!
//! The URL is written into an `href` by the search page, so `upsert`
//! refuses one that is not a relative path or `http(s)`: a `javascript:` URL
//! in the index would run in every visitor's browser. [`safe_url`] is the
//! rule, and the page checks it again on the way out.

use brocanth_policy::{Column, Entity, Ident};

use crate::index::{Index, Migration, Weight};

/// The table.
pub const TABLE: &str = "search_documents";

/// The columns of [`TABLE`], without the generated `search` column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DocumentColumn {
    /// `bigserial`, the primary key.
    Id,
    /// What kind of thing: `post`, `page`, `product`.
    Kind,
    /// Its key in its own table, as text.
    Key,
    /// The title, the most weighted.
    Title,
    /// The text to search.
    Body,
    /// Where the thing is.
    Url,
}

impl Column for DocumentColumn {
    const ALL: &'static [Self] = &[
        Self::Id,
        Self::Kind,
        Self::Key,
        Self::Title,
        Self::Body,
        Self::Url,
    ];

    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Kind => "kind",
            Self::Key => "key",
            Self::Title => "title",
            Self::Body => "body",
            Self::Url => "url",
        }
    }
}

/// The entity a policy over the site index is written for.
pub struct Document;

impl Entity for Document {
    type Column = DocumentColumn;
    const TABLE: Ident = Ident::must(TABLE);
    const PRIMARY_KEY: Self::Column = DocumentColumn::Id;
}

/// The search of the site index: title `A`, body `C`, English, and the
/// title by trigram too, so a typo in a title still finds it.
pub fn index() -> Index<Document> {
    Index::new("search")
        .source(DocumentColumn::Title, Weight::A)
        .source(DocumentColumn::Body, Weight::C)
        .trigram(DocumentColumn::Title)
}

/// The table, then [`index`]'s column and indexes.
pub fn migration() -> Migration {
    let index = index()
        .migration()
        .expect("the site index declares its columns");
    Migration {
        up: format!("{CREATE_TABLE}{}", index.up),
        down: format!("DROP TABLE \"{TABLE}\";\n"),
    }
}

const CREATE_TABLE: &str = "CREATE TABLE \"search_documents\" (
    \"id\" bigserial PRIMARY KEY,
    \"kind\" text NOT NULL,
    \"key\" text NOT NULL,
    \"title\" text NOT NULL,
    \"body\" text NOT NULL DEFAULT '',
    \"url\" text NOT NULL,
    UNIQUE (\"kind\", \"key\")
);
";

/// One row of the site index, with its rank in a search.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// The row's id.
    pub id: i64,
    /// What kind of thing.
    pub kind: String,
    /// Its key in its own table.
    pub key: String,
    /// The title.
    pub title: String,
    /// The text.
    pub body: String,
    /// Where it is.
    pub url: String,
    /// How well it matched. Higher is better; only the order means anything.
    pub search_rank: f32,
}

/// Whether `url` may go into an `href`: a relative path (`/posts/42`,
/// `posts/42`, `?page=2`) or an absolute `http://` or `https://` URL.
///
/// Everything else is refused: a scheme such as `javascript:` or `data:`, a
/// protocol-relative `//host`, a backslash a browser reads as a slash, and
/// any control character or leading space, which a browser strips before it
/// reads the scheme.
pub fn safe_url(url: &str) -> bool {
    if url.is_empty() || url.starts_with(char::is_whitespace) || url.chars().any(char::is_control) {
        return false;
    }
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return true;
    }
    if url.starts_with("//") || url.starts_with('\\') || url.starts_with("/\\") {
        return false;
    }
    // A scheme is a `:` before the first `/`, `?` or `#`.
    let head = url.split(['/', '?', '#']).next().unwrap_or_default();
    !head.contains(':')
}

impl Hit {
    /// The body cut to about `chars` characters, at a word, with `…` when
    /// it was cut.
    pub fn excerpt(&self, chars: usize) -> String {
        if self.body.chars().count() <= chars {
            return self.body.clone();
        }
        let cut: String = self.body.chars().take(chars).collect();
        let at = cut.rfind(char::is_whitespace).unwrap_or(cut.len());
        format!("{}…", cut[..at].trim_end())
    }
}

#[cfg(feature = "postgres")]
mod store {
    use brocanth_db::sqlx::postgres::PgRow;
    use brocanth_db::sqlx::{self, AssertSqlSafe, FromRow, Row};
    use brocanth_db::{DbError, Exec};

    use super::{safe_url, Hit};
    use crate::SearchError;

    impl<'r> FromRow<'r, PgRow> for Hit {
        fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
            Ok(Hit {
                id: row.try_get("id")?,
                kind: row.try_get("kind")?,
                key: row.try_get("key")?,
                title: row.try_get("title")?,
                body: row.try_get("body")?,
                url: row.try_get("url")?,
                search_rank: row.try_get("search_rank")?,
            })
        }
    }

    /// Writes one thing into the site index, or replaces its row.
    ///
    /// `kind` and `key` name the thing: `("post", "42")`. Call it when the
    /// thing is created or changed.
    ///
    /// `db` is a `&Db` (its own statement) or a `&mut Tx`: pass the
    /// transaction that writes the thing, and the row commits or rolls back
    /// with it.
    ///
    /// # Errors
    ///
    /// [`SearchError::UnsafeUrl`] for a URL [`safe_url`] refuses, before
    /// anything reaches the database; else what the database raised.
    pub async fn upsert(
        mut db: impl Exec,
        kind: &str,
        key: &str,
        title: &str,
        body: &str,
        url: &str,
    ) -> Result<(), SearchError> {
        if !safe_url(url) {
            return Err(SearchError::UnsafeUrl {
                url: url.to_string(),
            });
        }
        sqlx::query(AssertSqlSafe(
            "INSERT INTO \"search_documents\" (\"kind\", \"key\", \"title\", \"body\", \"url\") \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (\"kind\", \"key\") DO UPDATE SET \
             \"title\" = EXCLUDED.\"title\", \"body\" = EXCLUDED.\"body\", \"url\" = EXCLUDED.\"url\"",
        ))
        .bind(kind)
        .bind(key)
        .bind(title)
        .bind(body)
        .bind(url)
        .execute(db.executor())
        .await
        .map(|_| ())
        .map_err(|e| SearchError::Db(DbError::from(e)))
    }

    /// Removes one thing from the site index. Removing a thing that is not
    /// there is not an error. `db` is a `&Db` or a `&mut Tx`, as for
    /// [`upsert`].
    ///
    /// # Errors
    ///
    /// What the database raised.
    pub async fn remove(mut db: impl Exec, kind: &str, key: &str) -> Result<u64, DbError> {
        sqlx::query(AssertSqlSafe(
            "DELETE FROM \"search_documents\" WHERE \"kind\" = $1 AND \"key\" = $2",
        ))
        .bind(kind)
        .bind(key)
        .execute(db.executor())
        .await
        .map(|done| done.rows_affected())
        .map_err(DbError::from)
    }
}

#[cfg(feature = "postgres")]
pub use store::{remove, upsert};

#[cfg(test)]
mod tests {
    use super::safe_url;

    #[test]
    fn a_path_or_an_http_url_is_safe_in_an_href() {
        for url in [
            "/posts/42",
            "posts/42",
            "?page=2",
            "#top",
            "/a:b",
            "https://example.com/x",
            "HTTP://example.com",
        ] {
            assert!(safe_url(url), "{url}");
        }
    }

    #[test]
    fn a_script_url_is_refused() {
        for url in [
            "javascript:alert(1)",
            "JavaScript:alert(1)",
            " javascript:alert(1)",
            "java\tscript:alert(1)",
            "data:text/html,<script>",
            "vbscript:x",
            "//evil.example",
            "\\\\evil.example",
            "/\\evil.example",
            "",
        ] {
            assert!(!safe_url(url), "{url:?}");
        }
    }
}
