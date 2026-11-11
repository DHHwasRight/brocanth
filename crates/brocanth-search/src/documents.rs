//! The site index: one table of everything a visitor can find.
//!
//! pg_search calls it multisearch, django-watson calls it a search entry. A
//! post, a page and a product each write a row here when they change: a
//! kind, a key, a title, a body and the URL of the thing. One query then
//! searches them all, and the search page needs no join.
//!
//! The row is a copy, so it can go stale. Write it in the same transaction
//! as the thing it describes, or from a job, and delete it with the thing.

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
    use brocanth_db::{Db, DbError};

    use super::Hit;

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
    /// # Errors
    ///
    /// What the database raised, as a [`DbError`] with a `help:` line.
    pub async fn upsert(
        db: &Db,
        kind: &str,
        key: &str,
        title: &str,
        body: &str,
        url: &str,
    ) -> Result<(), DbError> {
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
        .execute(db.pool())
        .await
        .map(|_| ())
        .map_err(DbError::from)
    }

    /// Removes one thing from the site index. Removing a thing that is not
    /// there is not an error.
    ///
    /// # Errors
    ///
    /// What the database raised.
    pub async fn remove(db: &Db, kind: &str, key: &str) -> Result<u64, DbError> {
        sqlx::query(AssertSqlSafe(
            "DELETE FROM \"search_documents\" WHERE \"kind\" = $1 AND \"key\" = $2",
        ))
        .bind(kind)
        .bind(key)
        .execute(db.pool())
        .await
        .map(|done| done.rows_affected())
        .map_err(DbError::from)
    }
}

#[cfg(feature = "postgres")]
pub use store::{remove, upsert};
