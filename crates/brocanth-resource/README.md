# brocanth-resource

Typed CRUD routes over one table, with the policy in the `WHERE` clause.
You write a `Resource`: an entity, a `brocanth-policy` policy, a data source
that runs SQL, and three conversions (an id, a request body, a row). The crate
gives you list, view, create, update and delete, with the status codes of
PocketBase's records API. A `Resource` without a policy does not compile.
Use it for a JSON records API, or for the form routes behind your own pages.
Do not use it for dynamic collections, a string filter language, batch writes
or file upload: these are out of scope (PLAN.md §12). The crate has no
SQL driver, no `serde` and no async runtime. Your data source brings them.

## Getting started

```toml
[dependencies]
brocanth-resource = { git = "https://github.com/DHHwasRight/brocanth.git" }
# The `policy!` macro, to write the entity and its rules.
brocanth-policy = { git = "https://github.com/DHHwasRight/brocanth.git", features = ["macro"] }
```

Features:

| Feature | Default | What it adds |
|---|---|---|
| `plugin` | on | `ResourceRoutes`, a `RouteSource` for a host that mounts one |
| `pages` | on | `pages::mount`, for the one route table of a generated project |
| `scaffold` | off | the `resource` module for `brocanth create`. Only the CLI uses it |

With both `plugin` and `pages` off, the crate is the authorisation flow
(`flow`) and the renderer (`render`), and does not link `brocanth`.

### In a generated project

```sh
brocanth create blog --with resource
```

The module writes `src/plugins/resource/mod.rs` and mounts it at `/api` in
`src/pages/mod.rs`. That file makes the project's `Ctx`, `Response` and
`Error` the page triple this crate needs (`PageCtx`, `PageResponse`,
`PageError`). Its `mount` lists the resources, and the list starts empty:
the module writes no entity, because none can be guessed. Add one line per
resource:

```rust,ignore
brocanth_resource::pages::mount(routes, &format!("{prefix}/posts"), Arc::new(Posts::new()))?;
```

The module declares no settings: there is no `[resource]` section, flag or
variable. The resources are code.

`brocanth add resource` works in an existing project. The module changes
`src/pages/mod.rs`: if you have not edited that file, the command rewrites
it. If you have, it prints the change, writes nothing, and goes through
once you have made the change by hand.

## Examples

### A resource, and a list filtered in the query

One entity, one policy, a Postgres data source, and the five operations
driven through `flow`. The data source puts `query.condition()` into its
SQL. It never sees a row the policy refuses. The condition numbers its
placeholders from `$1`, so the data source numbers its own after them.

```rust
use brocanth_policy::{Column, Dialect, Value};
use brocanth_resource::data::{DataError, FetchQuery, ListQuery, ResourceData};
use brocanth_resource::sql::Statement;
use brocanth_resource::status::{outcome, Action};
use brocanth_resource::{flow, Failure, PageRequest, Resource};
# use sqlx::{AssertSqlSafe, Row as _};

pub struct Visitor {
    pub user_id: Option<i64>,
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
        status: String,
        title: String,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        /// Published posts, or your own.
        list: status == "published" || author_id == who.user_id else deny,
        view: status == "published" || author_id == who.user_id else deny,
        create: author_id == who.user_id else deny,
        update: author_id == who.user_id else deny,
        // No `delete` rule: it defaults to `never`, which is a 403.
        // An update changes the title only. A create also names the owner,
        // and `create` admits only the subject's own id. `update` reads the
        // stored row, so `author_id` in `writable` would let an author give
        // a post away.
        writable: only [title],
        writable_on_create: only [title, author_id],
    }
}

#[derive(Debug)]
pub struct PostRow {
    pub id: i64,
    pub title: String,
}

/// The data source. It holds a pool.
pub struct Db {
#     pool: sqlx::PgPool,
#     runtime: tokio::runtime::Runtime,
#     schema: String,
    // ...
}

impl ResourceData<Post> for Db {
    type Row = PostRow;

    fn count(&self, query: &ListQuery<'_, PostColumn>) -> Result<u64, DataError> {
        let sql = format!("SELECT count(*) FROM posts WHERE ({})", query.condition().sql());
        let rows = self.run(&sql, query.condition().binds().to_vec())?;
        Ok(rows[0].get::<i64, _>(0) as u64)
    }

    fn list(&self, query: &ListQuery<'_, PostColumn>) -> Result<Vec<PostRow>, DataError> {
        let sort = query.sort();
        let next = query.condition().binds().len() + 1;
        let sql = format!(
            "SELECT id, title FROM posts WHERE ({}) ORDER BY \"{}\" {} LIMIT ${} OFFSET ${}",
            query.condition().sql(),
            sort.column.name(),
            sort.direction.as_sql(),
            next,
            next + 1,
        );
        let mut binds = query.condition().binds().to_vec();
        binds.push(Value::Int(query.limit() as i64));
        binds.push(Value::Int(query.offset() as i64));
        Ok(self.run(&sql, binds)?.iter().map(post).collect())
    }

    fn fetch(&self, query: &FetchQuery<'_, PostColumn>) -> Result<Option<PostRow>, DataError> {
        let sql = format!(
            "SELECT id, title FROM posts WHERE id = ${} AND ({})",
            query.condition().binds().len() + 1,
            query.condition().sql()
        );
        let mut binds = query.condition().binds().to_vec();
        binds.push(query.id().clone());
        Ok(self.run(&sql, binds)?.iter().map(post).next())
    }

    /// The admission test before a write: how many rows the guard matches.
    fn admit(&self, statement: &Statement) -> Result<usize, DataError> {
        Ok(self.run(statement.sql(), statement.binds().to_vec())?.len())
    }

    /// `UPDATE … WHERE id = $n AND <condition>`. Zero rows is a lost race, a 409.
    fn write(&self, statement: &Statement) -> Result<u64, DataError> {
        self.execute(statement.sql(), statement.binds().to_vec())
    }

    fn insert(&self, values: &[(PostColumn, Value)]) -> Result<(Value, PostRow), DataError> {
        let names: Vec<&str> = values.iter().map(|(c, _)| c.name()).collect();
        let holes: Vec<String> = (1..=values.len()).map(|n| format!("${n}")).collect();
        let sql = format!(
            "INSERT INTO posts ({}) VALUES ({}) RETURNING id, title",
            names.join(", "),
            holes.join(", ")
        );
        let binds = values.iter().map(|(_, v)| v.clone()).collect();
        let row = post(&self.run(&sql, binds)?[0]);
        Ok((Value::Int(row.id), row))
    }
}
#
# fn post(row: &sqlx::postgres::PgRow) -> PostRow {
#     PostRow { id: row.get(0), title: row.get(1) }
# }
#
# type Query = sqlx::query::Query<'static, sqlx::Postgres, sqlx::postgres::PgArguments>;
#
# fn bind(mut query: Query, binds: Vec<Value>) -> Query {
#     for value in binds {
#         query = match value {
#             Value::Null => query.bind(Option::<i64>::None),
#             Value::Bool(b) => query.bind(b),
#             Value::Int(i) => query.bind(i),
#             Value::Float(f) => query.bind(f),
#             Value::Text(t) => query.bind(t),
#             Value::Bytes(b) => query.bind(b),
#         };
#     }
#     query
# }
#
# impl Db {
#     /// A schema of its own, or `None` when no database is set.
#     fn new() -> Option<Self> {
#         use std::str::FromStr as _;
#         let url = std::env::var("BROCANTH_TEST_DATABASE_URL").ok().filter(|u| !u.is_empty())?;
#         let schema = format!("brocanth_resource_readme_{}", std::process::id());
#         let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
#         let options = sqlx::postgres::PgConnectOptions::from_str(&url)
#             .unwrap()
#             .options([("search_path", schema.as_str())]);
#         let pool = runtime
#             .block_on(sqlx::postgres::PgPoolOptions::new().max_connections(1).connect_with(options))
#             .unwrap();
#         let db = Db { pool, runtime, schema };
#         db.execute(&format!("CREATE SCHEMA \"{}\"", db.schema), vec![]).unwrap();
#         db.execute(
#             "CREATE TABLE posts (id BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, \
#              author_id BIGINT, status TEXT NOT NULL DEFAULT 'draft', title TEXT)",
#             vec![],
#         ).unwrap();
#         Some(db)
#     }
#
#     fn run(&self, sql: &str, binds: Vec<Value>) -> Result<Vec<sqlx::postgres::PgRow>, DataError> {
#         let query = bind(sqlx::query(AssertSqlSafe(sql.to_owned())), binds);
#         self.runtime.block_on(query.fetch_all(&self.pool)).map_err(|e| DataError::backend(e.to_string()))
#     }
#
#     fn execute(&self, sql: &str, binds: Vec<Value>) -> Result<u64, DataError> {
#         let query = bind(sqlx::query(AssertSqlSafe(sql.to_owned())), binds);
#         self.runtime
#             .block_on(query.execute(&self.pool))
#             .map(|done| done.rows_affected())
#             .map_err(|e| DataError::backend(e.to_string()))
#     }
# }
#
# impl Drop for Db {
#     fn drop(&mut self) {
#         let _ = self.execute(&format!("DROP SCHEMA \"{}\" CASCADE", self.schema), vec![]);
#     }
# }

pub struct Posts {
    db: Db,
}

impl Resource for Posts {
    type Entity = Post;
    type Policy = PostPolicy; // No default: omit it and this does not compile.
    type Subject = Visitor;
    type Data = Db;

    const NAME: &'static str = "posts";
    const DIALECT: Dialect = Dialect::Postgres;

    fn policy(&self) -> &PostPolicy {
        &PostPolicy
    }

    fn data(&self) -> &Db {
        &self.db
    }

    fn parse_id(&self, raw: &str) -> Result<Value, Failure> {
        raw.parse()
            .map(Value::Int)
            .map_err(|_| Failure::bad_request(format!("`{raw}` is not a post id")))
    }

    /// `title=Hello&author_id=7`. The body format is yours.
    fn parse_write(&self, body: &[u8]) -> Result<Vec<(PostColumn, Value)>, Failure> {
        let text = std::str::from_utf8(body).map_err(|_| Failure::bad_request("not UTF-8"))?;
        let mut out = Vec::new();
        for (key, raw) in text.split('&').filter_map(|pair| pair.split_once('=')) {
            let column = PostColumn::ALL
                .iter()
                .copied()
                .find(|c| c.name() == key)
                .ok_or_else(|| Failure::bad_request(format!("`{key}` is not a column")))?;
            let value = match column {
                PostColumn::Id | PostColumn::AuthorId => {
                    Value::Int(raw.parse().map_err(|_| Failure::bad_request("not a number"))?)
                }
                _ => Value::Text(raw.to_owned()),
            };
            out.push((column, value));
        }
        Ok(out)
    }

    fn encode_row(&self, row: &PostRow, out: &mut Vec<u8>) -> Result<(), Failure> {
        out.extend_from_slice(format!("{{\"id\":{},", row.id).as_bytes());
        brocanth_resource::json::push_key(out, "title");
        brocanth_resource::json::push_string(out, &row.title);
        out.push(b'}');
        Ok(())
    }
}

# let Some(db) = Db::new() else { return Ok(()) }; // No database: nothing to show.
let posts = Posts { db };
# posts.db.execute(
#     "INSERT INTO posts (author_id, status, title) VALUES (7, 'published', 'Hello'), (8, 'draft', 'Secret')",
#     vec![],
# ).unwrap();
let ada = Visitor { user_id: Some(7) };
let anonymous = Visitor { user_id: None };

// Author 8's draft is not on anybody else's page, and not in the total.
let page = flow::list(&posts, &ada, &PageRequest::new(), None)?;
assert_eq!(page.items.len(), 1);
assert_eq!(page.total.count(), Some(1));

// A row you may not see is a 404, the same answer as a row that is not there.
let hidden = flow::view(&posts, &ada, "2").unwrap_err();
assert_eq!(outcome(Action::View, &hidden).status, 404);

// Anonymous visitors create nothing: with no user id the rule is `never`.
let refused = flow::create(&posts, &anonymous, b"title=Spam").unwrap_err();
assert_eq!(outcome(Action::Create, &refused).status, 403);

// A post for somebody else: the proposed row fails the rule, a 400.
let refused = flow::create(&posts, &ada, b"title=Spam&author_id=8").unwrap_err();
assert_eq!(outcome(Action::Create, &refused).status, 400);

// Your own post: created, then updated. Delete has no rule, so it is a 403.
let created = flow::create(&posts, &ada, b"title=Draft&author_id=7")?;
flow::update(&posts, &ada, &created.row.id.to_string(), b"title=Final")?;
let denied = flow::delete(&posts, &ada, "1").unwrap_err();
assert_eq!(outcome(Action::Delete, &denied).status, 403);
# Ok::<(), Failure>(())
```

### The routes

`flow` knows no HTTP. Two surfaces put it on a router and render through
`render`, so they answer with one status table:

- `pages::mount(routes, "/api/posts", Arc::new(posts))` adds seven routes to
  a generated project's table: `GET` and `POST` on `/`, `GET`, `PATCH` and
  `DELETE` on `/{id}`, and `POST /{id}` and `POST /{id}/delete` for an HTML
  form. `pages::mount_api` adds the five REST routes only. The resource also
  implements `PageSubject<Ctx>`, which reads who is asking off the request.
- `ResourceRoutes<R>` is a plugin and a `RouteSource` for an `App`. The
  resource implements `SubjectSource<E>`.

A list answers PocketBase's envelope:
`{"page":1,"perPage":30,"totalItems":2,"totalPages":1,"items":[…]}`.

### The status table

`status::outcome` maps a `Failure` to a status. The rule: a status must not
tell you about a record you may not see.

```rust
use brocanth_resource::status::{outcome, Action};
use brocanth_resource::Failure;

// A refused or missing row: one answer.
assert_eq!(outcome(Action::View, &Failure::NoMatch).status, 404);
// A list rule that matches nothing is an empty page, not a refusal.
assert_eq!(outcome(Action::List, &Failure::NoMatch).status, 200);
// A rule of `never`: a fact about the collection, not a row.
assert_eq!(outcome(Action::Update, &Failure::NoRule).status, 403);
// The guarded write changed zero rows.
assert_eq!(outcome(Action::Update, &Failure::RaceLost).status, 409);
```

## Testing

```sh
cargo test -p brocanth-resource
```

The tests that need a database run against Postgres through `sqlx`, because
the claim under test is that the policy reaches the SQL. Set
`BROCANTH_TEST_DATABASE_URL` to run them. Each test creates its own schema
and drops it at the end. Without the variable they print `!! SKIPPED` and
pass, and the tests that send no query still run.

`tests/common/mod.rs` is the full fixture: an entity, a policy, a data source
and a recording change sink. `tests/ui/` holds the compile-fail cases (a
resource with no policy, or a policy for another entity), run by
`tests/trybuild.rs`.

To test your own resource, drive `flow` with each subject, as the example
above does, and check the status with `status::outcome`. To test the routes,
build the route table and dispatch a request, as `tests/pages.rs` does for
the page surface and `tests/routes.rs` does for `ResourceRoutes`.

## Links

- `docs/pocketbase-surface.md`: the records API and the rules this crate
  answers (§2, §3).
- `docs/policy.md`: the policy algebra, `Verified` and the field mask.
- `docs/modules.md`: how the `resource` module reaches a generated project.
- PLAN.md §9 (list authorisation) and §12 item 5.
