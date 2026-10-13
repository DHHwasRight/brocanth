//! One entity, one policy, one data source and a recording sink.
//!
//! The data source is a real SQLite database. The central claim of this crate
//! is that the policy is applied **in the query**, and a fixture that filtered
//! rows in Rust would prove only that the fixture and the crate agree about a
//! predicate neither of them ever sent to a database. `brocanth-policy`'s
//! agreement tests make the same argument and the same choice.

#![allow(dead_code)]

use std::sync::Mutex;

use brocanth_events::{ChangeSink, RowChange};
use brocanth_policy::{
    Bound, Column, Dialect, Entity, EntityPred, FieldMask, Ident, Policy, Pred, Value,
};
use brocanth_resource::data::{DataError, FetchQuery, ListQuery, ResourceData};
use brocanth_resource::sql::Statement;
use brocanth_resource::{ColumnOf, Failure, Resource};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::{AssertSqlSafe, Row as _};

// ---------------------------------------------------------------------------
// The entity
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostColumn {
    Id,
    AuthorId,
    Status,
    Title,
}

impl Column for PostColumn {
    const ALL: &'static [Self] = &[Self::Id, Self::AuthorId, Self::Status, Self::Title];

    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::AuthorId => "author_id",
            Self::Status => "status",
            Self::Title => "title",
        }
    }
}

pub struct Post;

impl Entity for Post {
    type Column = PostColumn;
    const TABLE: Ident = Ident::must("posts");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}

pub const SCHEMA: &str = "
    CREATE TABLE posts (
        id        INTEGER PRIMARY KEY AUTOINCREMENT,
        author_id INTEGER,
        status    TEXT,
        title     TEXT
    );
";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostRow {
    pub id: i64,
    pub author_id: Option<i64>,
    pub status: String,
    pub title: String,
}

// ---------------------------------------------------------------------------
// The subject and the policy
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
pub struct Visitor {
    pub user_id: Option<i64>,
    pub admin: bool,
    /// Nothing at all, not even the published rows: the typed form of
    /// PocketBase's `null` rule.
    pub banned: bool,
}

impl Visitor {
    pub fn anonymous() -> Self {
        Visitor::default()
    }

    pub fn user(id: i64) -> Self {
        Visitor {
            user_id: Some(id),
            admin: false,
            banned: false,
        }
    }

    pub fn admin() -> Self {
        Visitor {
            user_id: Some(0),
            admin: true,
            banned: false,
        }
    }

    pub fn banned() -> Self {
        Visitor {
            user_id: Some(7),
            admin: false,
            banned: true,
        }
    }
}

pub struct PostPolicy;

impl Policy for PostPolicy {
    type Entity = Post;
    type Subject = Visitor;

    /// Published posts, or your own.
    fn list(&self, who: &Visitor) -> EntityPred<Post> {
        if who.banned {
            return Pred::Never;
        }
        if who.admin {
            return Pred::Always;
        }
        Pred::any([
            Pred::eq(PostColumn::Status, "published"),
            Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id)),
        ])
    }

    /// Anonymous visitors create nothing, which is `Pred::Never` and a 403.
    fn create(&self, who: &Visitor) -> EntityPred<Post> {
        match who.user_id {
            Some(id) => Pred::eq(PostColumn::AuthorId, id),
            None => Pred::Never,
        }
    }

    fn update(&self, who: &Visitor) -> EntityPred<Post> {
        if who.admin {
            return Pred::Always;
        }
        Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id))
    }

    /// Only an administrator deletes. For everyone else the door is shut
    /// without reading a row.
    fn delete(&self, who: &Visitor) -> EntityPred<Post> {
        if who.admin {
            Pred::Always
        } else {
            Pred::Never
        }
    }

    /// An author writes the title and the author column. Never the status:
    /// PocketBase's `status:changed = false || role = "admin"`, as a value.
    fn writable(&self, who: &Visitor) -> FieldMask<PostColumn> {
        if who.admin {
            FieldMask::except([PostColumn::Id])
        } else {
            FieldMask::only([PostColumn::Title, PostColumn::AuthorId])
        }
    }
}

// ---------------------------------------------------------------------------
// The sink
// ---------------------------------------------------------------------------

/// Records what was published, and whether anybody was listening.
pub struct Recorder {
    pub listening: bool,
    published: Mutex<Vec<String>>,
    asked: Mutex<usize>,
}

impl Recorder {
    pub fn listening() -> Self {
        Recorder {
            listening: true,
            published: Mutex::new(Vec::new()),
            asked: Mutex::new(0),
        }
    }

    pub fn deaf() -> Self {
        Recorder {
            listening: false,
            published: Mutex::new(Vec::new()),
            asked: Mutex::new(0),
        }
    }

    pub fn published(&self) -> Vec<String> {
        self.published.lock().unwrap().clone()
    }

    pub fn asked(&self) -> usize {
        *self.asked.lock().unwrap()
    }
}

impl ChangeSink for Recorder {
    fn publish(&self, change: &RowChange<'_>) {
        let fields: Vec<&str> = change.changed_fields().iter().collect();
        let kind = match &change.kind {
            brocanth_events::ChangeKind::Created => "created".to_owned(),
            brocanth_events::ChangeKind::Updated { .. } => format!("updated[{}]", fields.join(",")),
            brocanth_events::ChangeKind::Deleted => "deleted".to_owned(),
        };
        self.published
            .lock()
            .unwrap()
            .push(format!("{}:{}:{}", change.collection, change.row, kind));
    }

    fn wants(&self, _collection: &str) -> bool {
        *self.asked.lock().unwrap() += 1;
        self.listening
    }
}

// ---------------------------------------------------------------------------
// The data source
// ---------------------------------------------------------------------------

type SqliteQuery = sqlx::query::Query<'static, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>;

fn bind_all(mut query: SqliteQuery, binds: &[Value]) -> SqliteQuery {
    for value in binds {
        query = match value {
            Value::Null => query.bind(Option::<i64>::None),
            Value::Bool(b) => query.bind(*b),
            Value::Int(i) => query.bind(*i),
            Value::Float(f) => query.bind(*f),
            Value::Text(t) => query.bind(t.clone()),
            Value::Bytes(b) => query.bind(b.clone()),
        };
    }
    query
}

/// An in-memory SQLite database, and a note of what it was asked.
pub struct Db {
    pool: SqlitePool,
    runtime: tokio::runtime::Runtime,
    /// Conditions this source was handed, newest last. The tests read it to
    /// assert that the policy reached the `WHERE` clause.
    pub seen: Mutex<Vec<String>>,
    /// A statement to run inside `admit`, after the count is taken: another
    /// transaction committing between the check and the write.
    pub race: Mutex<Option<(String, Vec<Value>)>>,
}

impl Db {
    pub fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let pool = runtime.block_on(async {
            // One connection: `sqlite::memory:` gives each connection its own
            // database, and a pool of two would hold two empty halves.
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .expect("an in-memory database");
            for statement in SCHEMA.split(';').filter(|s| !s.trim().is_empty()) {
                sqlx::query(AssertSqlSafe(statement.to_owned()))
                    .execute(&pool)
                    .await
                    .expect("the schema applies");
            }
            pool
        });
        Db {
            pool,
            runtime,
            seen: Mutex::new(Vec::new()),
            race: Mutex::new(None),
        }
    }

    pub fn execute(&self, sql: &str, binds: &[Value]) -> u64 {
        self.runtime.block_on(async {
            bind_all(sqlx::query(AssertSqlSafe(sql.to_owned())), binds)
                .execute(&self.pool)
                .await
                .expect("the statement runs")
                .rows_affected()
        })
    }

    pub fn seed(&self, rows: &[(i64, Option<i64>, &str, &str)]) {
        for (id, author, status, title) in rows {
            self.execute(
                "INSERT INTO posts (id, author_id, status, title) VALUES (?, ?, ?, ?)",
                &[
                    Value::Int(*id),
                    author.map(Value::Int).unwrap_or(Value::Null),
                    Value::Text((*status).to_owned()),
                    Value::Text((*title).to_owned()),
                ],
            );
        }
    }

    pub fn status_of(&self, id: i64) -> Option<String> {
        self.runtime.block_on(async {
            sqlx::query(AssertSqlSafe(
                "SELECT status FROM posts WHERE id = ?".to_owned(),
            ))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .expect("the statement runs")
            .map(|row| row.get::<String, _>(0))
        })
    }

    fn note(&self, condition: &str) {
        self.seen.lock().unwrap().push(condition.to_owned());
    }

    fn rows(&self, sql: String, binds: Vec<Value>) -> Result<Vec<PostRow>, DataError> {
        self.runtime.block_on(async {
            let rows = bind_all(sqlx::query(AssertSqlSafe(sql.clone())), &binds)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| DataError::backend(format!("{e} -- {sql}")))?;
            Ok(rows
                .iter()
                .map(|row| PostRow {
                    id: row.get(0),
                    author_id: row.get(1),
                    status: row.get(2),
                    title: row.get(3),
                })
                .collect())
        })
    }
}

const COLUMNS: &str = "id, author_id, status, title";

impl ResourceData<Post> for Db {
    type Row = PostRow;

    fn count(&self, query: &ListQuery<'_, PostColumn>) -> Result<u64, DataError> {
        self.note(query.condition().sql());
        let sql = format!(
            "SELECT count(*) FROM posts WHERE ({})",
            query.condition().sql()
        );
        self.runtime.block_on(async {
            let row = bind_all(
                sqlx::query(AssertSqlSafe(sql.clone())),
                query.condition().binds(),
            )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| DataError::backend(format!("{e} -- {sql}")))?;
            Ok(row.get::<i64, _>(0) as u64)
        })
    }

    fn list(&self, query: &ListQuery<'_, PostColumn>) -> Result<Vec<PostRow>, DataError> {
        self.note(query.condition().sql());
        let sort = query.sort();
        let sql = format!(
            "SELECT {COLUMNS} FROM posts WHERE ({}) ORDER BY \"{}\" {} LIMIT ? OFFSET ?",
            query.condition().sql(),
            sort.column.name(),
            sort.direction.as_sql(),
        );
        let mut binds = query.condition().binds().to_vec();
        binds.push(Value::Int(query.limit() as i64));
        binds.push(Value::Int(query.offset() as i64));
        self.rows(sql, binds)
    }

    fn fetch(&self, query: &FetchQuery<'_, PostColumn>) -> Result<Option<PostRow>, DataError> {
        self.note(query.condition().sql());
        let sql = format!(
            "SELECT {COLUMNS} FROM posts WHERE id = ? AND ({})",
            query.condition().sql()
        );
        let mut binds = vec![query.id().clone()];
        binds.extend(query.condition().binds().iter().cloned());
        Ok(self.rows(sql, binds)?.into_iter().next())
    }

    fn admit(&self, statement: &Statement) -> Result<usize, DataError> {
        let sql = statement.sql().to_owned();
        let count = self.runtime.block_on(async {
            bind_all(sqlx::query(AssertSqlSafe(sql.clone())), statement.binds())
                .fetch_all(&self.pool)
                .await
                .map(|rows| rows.len())
                .map_err(|e| DataError::backend(format!("{e} -- {sql}")))
        })?;
        // The other transaction, committing between the check and the write.
        let race = self.race.lock().unwrap().take();
        if let Some((sql, binds)) = race {
            self.execute(&sql, &binds);
        }
        Ok(count)
    }

    fn write(&self, statement: &Statement) -> Result<u64, DataError> {
        let sql = statement.sql().to_owned();
        self.runtime.block_on(async {
            bind_all(sqlx::query(AssertSqlSafe(sql.clone())), statement.binds())
                .execute(&self.pool)
                .await
                .map(|done| done.rows_affected())
                .map_err(|e| DataError::backend(format!("{e} -- {sql}")))
        })
    }

    fn insert(&self, values: &[(PostColumn, Value)]) -> Result<(Value, PostRow), DataError> {
        let names: Vec<String> = values
            .iter()
            .map(|(c, _)| format!("\"{}\"", c.name()))
            .collect();
        let holes: Vec<&str> = values.iter().map(|_| "?").collect();
        let sql = format!(
            "INSERT INTO posts ({}) VALUES ({}) RETURNING {COLUMNS}",
            names.join(", "),
            holes.join(", "),
        );
        let binds: Vec<Value> = values.iter().map(|(_, v)| v.clone()).collect();
        let row = self
            .rows(sql, binds)?
            .into_iter()
            .next()
            .ok_or_else(|| DataError::backend("the insert returned no row"))?;
        Ok((Value::Int(row.id), row))
    }
}

// ---------------------------------------------------------------------------
// The resource
// ---------------------------------------------------------------------------

pub struct Posts {
    pub db: Db,
    pub sink: Recorder,
}

impl Posts {
    pub fn new() -> Self {
        Posts {
            db: Db::new(),
            sink: Recorder::listening(),
        }
    }

    pub fn deaf() -> Self {
        Posts {
            db: Db::new(),
            sink: Recorder::deaf(),
        }
    }
}

impl Resource for Posts {
    type Entity = Post;
    type Policy = PostPolicy;
    type Subject = Visitor;
    type Data = Db;

    const NAME: &'static str = "posts";
    const PREFIX: &'static str = "/posts";
    const DIALECT: Dialect = Dialect::Sqlite;
    const DEFAULT_PER_PAGE: u64 = 30;
    const MAX_PER_PAGE: u64 = 100;

    fn policy(&self) -> &PostPolicy {
        &PostPolicy
    }

    fn data(&self) -> &Db {
        &self.db
    }

    fn sink(&self) -> &dyn ChangeSink {
        &self.sink
    }

    /// Integer keys, and a segment that is not one is a `400`.
    fn parse_id(&self, raw: &str) -> Result<Value, Failure> {
        raw.parse::<i64>()
            .map(Value::Int)
            .map_err(|_| Failure::bad_request(format!("`{raw}` is not a post id")))
    }

    /// `title=hello&status=draft`. The body format is the application's
    /// business, and form pairs keep these tests about authorisation rather
    /// than about a parser.
    fn parse_write(&self, body: &[u8]) -> Result<Vec<(PostColumn, Value)>, Failure> {
        let text =
            std::str::from_utf8(body).map_err(|_| Failure::bad_request("the body is not UTF-8"))?;
        let mut out = Vec::new();
        for pair in text.split('&').filter(|p| !p.is_empty()) {
            let (key, raw) = pair
                .split_once('=')
                .ok_or_else(|| Failure::bad_request(format!("`{pair}` is not a field")))?;
            let column = PostColumn::ALL
                .iter()
                .copied()
                .find(|c| c.name() == key)
                .ok_or_else(|| Failure::bad_request(format!("`{key}` is not a column")))?;
            let value = match column {
                PostColumn::Id | PostColumn::AuthorId => Value::Int(
                    raw.parse()
                        .map_err(|_| Failure::bad_request(format!("`{key}` wants a number")))?,
                ),
                _ => Value::Text(raw.to_owned()),
            };
            out.push((column, value));
        }
        Ok(out)
    }

    fn encode_row(&self, row: &PostRow, out: &mut Vec<u8>) -> Result<(), Failure> {
        let author = match row.author_id {
            Some(id) => id.to_string(),
            None => "null".to_owned(),
        };
        out.extend_from_slice(format!("{{\"id\":{},\"author_id\":{author},", row.id).as_bytes());
        brocanth_resource::json::push_key(out, "status");
        brocanth_resource::json::push_string(out, &row.status);
        out.push(b',');
        brocanth_resource::json::push_key(out, "title");
        brocanth_resource::json::push_string(out, &row.title);
        out.push(b'}');
        Ok(())
    }

    fn sort_column(&self, name: &str) -> Option<ColumnOf<Self>> {
        match name {
            "id" => Some(PostColumn::Id),
            "title" => Some(PostColumn::Title),
            _ => None,
        }
    }
}

/// The rows every listing test seeds: three published, three drafts, two
/// authors.
pub fn seed_eight(posts: &Posts) {
    posts.db.seed(&[
        (1, Some(7), "published", "one"),
        (2, Some(7), "draft", "two"),
        (3, Some(8), "published", "three"),
        (4, Some(8), "draft", "four"),
        (5, Some(8), "draft", "five"),
        (6, Some(9), "published", "six"),
        (7, Some(9), "draft", "seven"),
        (8, Some(9), "draft", "eight"),
    ]);
}

pub fn row_ids(items: &[PostRow]) -> Vec<i64> {
    items.iter().map(|row| row.id).collect()
}

// ---------------------------------------------------------------------------
// The same resource, with the other create interpretation
// ---------------------------------------------------------------------------

/// `Posts`, deciding its create rule in memory rather than in the database.
///
/// PLAN.md §9 obligation 1 makes that a choice a resource writes down, and
/// taking it takes on the obligation to prove the two interpretations agree.
/// Here the proof is cheap: `tests/create.rs` asks both of them the same
/// question and asserts the same answer.
pub struct InMemoryPosts(pub Posts);

impl InMemoryPosts {
    pub fn new() -> Self {
        InMemoryPosts(Posts::new())
    }
}

impl Resource for InMemoryPosts {
    type Entity = Post;
    type Policy = PostPolicy;
    type Subject = Visitor;
    type Data = Db;

    const NAME: &'static str = "posts";
    const DIALECT: Dialect = Dialect::Sqlite;
    const CREATE_CHECK: brocanth_resource::CreateCheck = brocanth_resource::CreateCheck::InMemory;

    fn policy(&self) -> &PostPolicy {
        self.0.policy()
    }

    fn data(&self) -> &Db {
        self.0.data()
    }

    fn sink(&self) -> &dyn ChangeSink {
        self.0.sink()
    }

    fn parse_id(&self, raw: &str) -> Result<Value, Failure> {
        self.0.parse_id(raw)
    }

    fn parse_write(&self, body: &[u8]) -> Result<Vec<(PostColumn, Value)>, Failure> {
        self.0.parse_write(body)
    }

    fn encode_row(&self, row: &PostRow, out: &mut Vec<u8>) -> Result<(), Failure> {
        self.0.encode_row(row, out)
    }
}
