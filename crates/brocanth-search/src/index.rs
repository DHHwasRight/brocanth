//! The declaration of one table's search: which columns, which weights,
//! which language, and the SQL it writes.

use std::marker::PhantomData;

use brocanth_policy::{Column, Condition, Entity, Ident, Value};

use crate::SearchError;

/// How much a match in a column counts. `A` is the most.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weight {
    /// A title.
    A,
    /// A summary.
    B,
    /// A body.
    C,
    /// Everything else.
    D,
}

impl Weight {
    fn letter(self) -> &'static str {
        match self {
            Weight::A => "A",
            Weight::B => "B",
            Weight::C => "C",
            Weight::D => "D",
        }
    }
}

/// A migration's two halves, as SQL text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Migration {
    /// The statements that apply it.
    pub up: String,
    /// The statements that undo it.
    pub down: String,
}

impl Migration {
    /// One `.sql` file, as the `db` module's `migrate` command reads it:
    /// `up`, a `-- down` line, `down`.
    pub fn file(&self) -> String {
        format!("{}\n-- down\n{}", self.up, self.down)
    }
}

/// A statement this crate wrote: SQL text and its binds, for Postgres.
///
/// The text comes from this crate and from `brocanth-policy`'s compiler.
/// Every value in it, the search term included, is a bind.
#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    /// The SQL, with `$1`, `$2` placeholders.
    pub sql: String,
    /// The values, in placeholder order.
    pub binds: Vec<Value>,
}

/// The two statements of one search: the page and the count. Both carry the
/// same `WHERE`, so the total counts the rows the pages show.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchQuery {
    /// The ranked page.
    pub select: Statement,
    /// `count(*)` over the same `WHERE`.
    pub count: Statement,
}

/// The longest search term this crate sends, in characters. A longer term
/// is cut, not refused: nobody types 200 characters into a search box, and
/// a pasted page should still search for its start.
pub const MAX_TERM: usize = 200;

/// The largest `LIMIT` or `OFFSET` Postgres takes.
const BIGINT_MAX: u64 = i64::MAX as u64;

/// The search of one table.
///
/// The column is a generated `tsvector`, so Postgres keeps it in step with
/// the source columns on every write, and a GIN index makes the match one
/// index scan. See [`Index::migration`].
#[derive(Clone, Debug)]
pub struct Index<E: Entity> {
    column: Ident,
    language: Ident,
    sources: Vec<(E::Column, Weight)>,
    trigram: Vec<E::Column>,
    marker: PhantomData<fn() -> E>,
}

impl<E: Entity> Index<E> {
    /// The search of `E`'s table, in the generated column `column`, in
    /// English.
    ///
    /// # Panics
    ///
    /// When `column` is not a bare SQL identifier. It is program text, so
    /// this fails on the first run, not on a user's input.
    pub fn new(column: &'static str) -> Self {
        Index {
            column: ident(column),
            language: Ident::must("english"),
            sources: Vec::new(),
            trigram: Vec::new(),
            marker: PhantomData,
        }
    }

    /// The text search configuration: `english`, `french`, `simple` (no
    /// stemming). One per index: the column and the query must agree.
    ///
    /// # Panics
    ///
    /// When `regconfig` is not a bare SQL identifier.
    pub fn language(mut self, regconfig: &'static str) -> Self {
        self.language = ident(regconfig);
        self
    }

    /// A column whose words are searched, and how much a match counts.
    pub fn source(mut self, column: E::Column, weight: Weight) -> Self {
        self.sources.push((column, weight));
        self
    }

    /// A column that also matches by trigram similarity, so a typo still
    /// finds it: `pg_trgm`'s `%`. Use it for a short column: a title, a name.
    pub fn trigram(mut self, column: E::Column) -> Self {
        self.trigram.push(column);
        self
    }

    /// The generated column, its GIN index and the trigram indexes.
    ///
    /// ```sql
    /// ALTER TABLE "posts" ADD COLUMN "search" tsvector GENERATED ALWAYS AS (
    ///     setweight(to_tsvector('english', coalesce("title", '')), 'A') || ...
    /// ) STORED;
    /// CREATE INDEX "posts_search_idx" ON "posts" USING gin ("search");
    /// ```
    ///
    /// `pg_trgm` is created when a column is declared with
    /// [`Index::trigram`]. It is a trusted extension since Postgres 13, so
    /// the database owner can create it.
    ///
    /// # Errors
    ///
    /// [`SearchError::NoSource`] when no column was declared, and
    /// [`SearchError::Compile`] for a column name that is not a bare SQL
    /// identifier.
    pub fn migration(&self) -> Result<Migration, SearchError> {
        let table = E::TABLE.as_str();
        let column = self.column.as_str();
        let mut up = String::new();
        if !self.trigram.is_empty() {
            up.push_str("CREATE EXTENSION IF NOT EXISTS pg_trgm;\n");
        }
        up.push_str(&format!(
            "ALTER TABLE \"{table}\" ADD COLUMN \"{column}\" tsvector GENERATED ALWAYS AS (\n    {}\n) STORED;\n",
            self.vector()?
        ));
        up.push_str(&format!(
            "CREATE INDEX \"{table}_{column}_idx\" ON \"{table}\" USING gin (\"{column}\");\n"
        ));
        let mut down = String::new();
        for source in &self.trigram {
            let name = source.ident()?.as_str();
            up.push_str(&format!(
                "CREATE INDEX \"{table}_{name}_trgm_idx\" ON \"{table}\" USING gin (\"{name}\" gin_trgm_ops);\n"
            ));
            down.push_str(&format!("DROP INDEX \"{table}_{name}_trgm_idx\";\n"));
        }
        down.push_str(&format!("DROP INDEX \"{table}_{column}_idx\";\n"));
        down.push_str(&format!(
            "ALTER TABLE \"{table}\" DROP COLUMN \"{column}\";\n"
        ));
        Ok(Migration { up, down })
    }

    /// `setweight(to_tsvector('english', coalesce("title", '')), 'A') || …`
    pub(crate) fn vector(&self) -> Result<String, SearchError> {
        if self.sources.is_empty() {
            return Err(SearchError::NoSource {
                table: E::TABLE.as_str(),
            });
        }
        let language = self.language.as_str();
        let mut parts = Vec::new();
        for (column, weight) in &self.sources {
            parts.push(format!(
                "setweight(to_tsvector('{language}', coalesce(\"{}\", '')), '{}')",
                column.ident()?.as_str(),
                weight.letter()
            ));
        }
        Ok(parts.join(" ||\n    "))
    }

    /// The ranked page and its count, for `term`, inside `condition`.
    ///
    /// `condition` is the policy's rule (and the filters) compiled for
    /// Postgres. It goes first in the `WHERE`; the match is ANDed after it.
    /// A row the rule refuses is never ranked, never counted, never shown.
    ///
    /// The rows come in rank order, best first, then by primary key, so two
    /// pages never share a row. The select lists every column of `E` and
    /// `search_rank`, a `real`.
    ///
    /// The term is read by `websearch_to_tsquery`: words, `"a phrase"`,
    /// `-not`, `or`. It never fails to parse.
    ///
    /// # Errors
    ///
    /// [`SearchError::NoSource`] and [`SearchError::Compile`].
    pub fn query(
        &self,
        condition: &Condition,
        term: &str,
        offset: u64,
        limit: u64,
    ) -> Result<SearchQuery, SearchError> {
        let _ = self.vector()?;
        let table = E::TABLE.as_str();
        let language = self.language.as_str();
        let vector = format!("\"{table}\".\"{}\"", self.column.as_str());
        let mut binds = condition.binds().to_vec();
        // A control character becomes a space. Postgres refuses NUL in a
        // text value, and `?q=%00` is one keystroke away; the others are
        // never part of a word.
        binds.push(Value::Text(
            term.chars()
                .take(MAX_TERM)
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect(),
        ));
        let term = format!("${}", binds.len());
        let query = format!("websearch_to_tsquery('{language}', {term})");

        let mut matches = format!("{vector} @@ {query}");
        let mut rank = format!("ts_rank_cd({vector}, {query})");
        for column in &self.trigram {
            let name = format!("\"{table}\".\"{}\"", column.ident()?.as_str());
            matches.push_str(&format!(" OR {name} % {term}"));
            rank.push_str(&format!(" + similarity({name}, {term})"));
        }
        let filter = format!("({}) AND ({matches})", condition.sql());

        let mut columns = Vec::new();
        for column in E::Column::ALL {
            columns.push(format!("\"{table}\".\"{}\"", column.ident()?.as_str()));
        }
        let key = E::PRIMARY_KEY.ident()?.as_str();
        // `LIMIT` and `OFFSET` are `bigint`: a `u64` above `i64::MAX` is
        // "out of range", so a page size of `u64::MAX` means "all".
        let (limit, offset) = (limit.min(BIGINT_MAX), offset.min(BIGINT_MAX));
        let select = format!(
            "SELECT {}, ({rank})::real AS search_rank FROM \"{table}\" WHERE {filter} \
             ORDER BY search_rank DESC, \"{table}\".\"{key}\" ASC LIMIT {limit} OFFSET {offset}",
            columns.join(", ")
        );
        let count = format!("SELECT count(*) FROM \"{table}\" WHERE {filter}");
        Ok(SearchQuery {
            select: Statement {
                sql: select,
                binds: binds.clone(),
            },
            count: Statement { sql: count, binds },
        })
    }
}

fn ident(name: &'static str) -> Ident {
    match Ident::new(name) {
        Ok(ident) => ident,
        Err(_) => panic!(
            "`{name}` is not a bare SQL identifier\n\
             help: use letters, digits and `_`, starting with a letter"
        ),
    }
}
