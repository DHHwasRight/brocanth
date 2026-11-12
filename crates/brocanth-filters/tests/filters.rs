//! Filters narrow the rows in the database, beside the rule, and a query
//! string the declaration refuses is an error with a `help:` line.
//!
//! The database tests run on Postgres, each in a schema of its own. Without
//! `BROCANTH_TEST_DATABASE_URL` they print `!! SKIPPED` and pass.

use brocanth_filters::{Direction, FilterError, Filters, Lookup};
use brocanth_policy::{Bound, Column, Dialect, Entity, Ident, Pred, Value};
use std::str::FromStr as _;
use std::sync::atomic::{AtomicU32, Ordering};

use sqlx::postgres::{PgConnectOptions, PgConnection, PgPool, PgPoolOptions};
use sqlx::{AssertSqlSafe, Connection as _, Row};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PostColumn {
    Id,
    AuthorId,
    Status,
    Score,
    Featured,
    ClosedAt,
}

impl Column for PostColumn {
    const ALL: &'static [Self] = &[
        Self::Id,
        Self::AuthorId,
        Self::Status,
        Self::Score,
        Self::Featured,
        Self::ClosedAt,
    ];

    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::AuthorId => "author_id",
            Self::Status => "status",
            Self::Score => "score",
            Self::Featured => "featured",
            Self::ClosedAt => "closed_at",
        }
    }
}

struct Post;

impl Entity for Post {
    type Column = PostColumn;
    const TABLE: Ident = Ident::must("posts");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}

fn declared() -> Filters<PostColumn> {
    Filters::new()
        .choice("status", PostColumn::Status, &["draft", "published"])
        .int("author", PostColumn::AuthorId)
        .int("score", PostColumn::Score)
        .boolean("featured", PostColumn::Featured)
        .text("closed_at", PostColumn::ClosedAt)
        .nullable()
        .only(&[Lookup::Null])
        .sort("score", PostColumn::Score)
        .default_sort("id", PostColumn::Id, Direction::Ascending)
}

/// The database URL, or `None` when no database is set.
fn url() -> Option<String> {
    std::env::var("BROCANTH_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

/// Says on the real stderr that the test did not run. Cargo captures
/// `eprintln!`, so the line goes to the stream directly.
fn skipped() {
    use std::io::Write as _;
    let name = std::thread::current().name().unwrap_or("a test").to_owned();
    let _ = std::io::stderr()
        .write_all(format!("!! SKIPPED {name}: set BROCANTH_TEST_DATABASE_URL\n").as_bytes());
}

/// A pool on a schema of its own. The schema is dropped with the value.
struct Db {
    pool: PgPool,
    url: String,
    schema: String,
}

impl Drop for Db {
    /// A thread and a runtime of its own: the test's runtime cannot block,
    /// and the schema goes even when the test panics.
    fn drop(&mut self) {
        let url = self.url.clone();
        let sql = format!("DROP SCHEMA IF EXISTS \"{}\" CASCADE", self.schema);
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a runtime");
            runtime.block_on(async {
                if let Ok(mut admin) = PgConnection::connect(&url).await {
                    let _ = sqlx::query(AssertSqlSafe(sql)).execute(&mut admin).await;
                    let _ = admin.close().await;
                }
            });
        })
        .join();
    }
}

/// Twelve posts: three authors, two statuses, scores 0 to 11, every third
/// one featured, every fourth one closed. `None`, after a skip line, when no
/// database is set.
async fn database() -> Option<Db> {
    static N: AtomicU32 = AtomicU32::new(0);
    let Some(url) = url() else {
        skipped();
        return None;
    };
    let schema = format!(
        "brocanth_filters_t{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let mut admin = PgConnection::connect(&url).await.expect("a database");
    sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA \"{schema}\"")))
        .execute(&mut admin)
        .await
        .expect("the schema is created");
    admin.close().await.expect("the connection closes");
    let options = PgConnectOptions::from_str(&url)
        .expect("a database URL")
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("a pool");
    let db = Db { pool, url, schema };
    sqlx::query(
        "CREATE TABLE posts (id BIGINT PRIMARY KEY, author_id BIGINT, status TEXT, \
         score BIGINT, featured BOOLEAN, closed_at TEXT)",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    for id in 1..=12_i64 {
        sqlx::query("INSERT INTO posts VALUES ($1, $2, $3, $4, $5, $6)")
            .bind(id)
            .bind(id % 3)
            .bind(if id % 2 == 0 { "published" } else { "draft" })
            .bind(id - 1)
            .bind(id % 3 == 0)
            .bind((id % 4 == 0).then_some("2026-11-09"))
            .execute(&db.pool)
            .await
            .unwrap();
    }
    Some(db)
}

/// The ids the database returns for the rule AND the filters, in the
/// order the filters ask for, and the count of the same `WHERE`.
async fn ids(db: &Db, rule: Pred<PostColumn>, query: &str) -> (Vec<i64>, i64) {
    let applied = declared().parse(query).expect("a query the list accepts");
    let condition = applied
        .condition::<Post>(rule, Dialect::Postgres)
        .expect("compiles");
    let order = applied.order_by::<Post>(PostColumn::Id).expect("orders");
    let sql = format!(
        "SELECT \"posts\".\"id\" FROM \"posts\" WHERE {} ORDER BY {order}",
        condition.sql()
    );
    let count_sql = format!("SELECT count(*) FROM \"posts\" WHERE {}", condition.sql());
    let mut select = sqlx::query(AssertSqlSafe(sql));
    let mut count = sqlx::query(AssertSqlSafe(count_sql));
    for value in condition.binds() {
        (select, count) = match value {
            Value::Int(n) => (select.bind(*n), count.bind(*n)),
            Value::Text(t) => (select.bind(t.clone()), count.bind(t.clone())),
            Value::Bool(b) => (select.bind(*b), count.bind(*b)),
            Value::Float(f) => (select.bind(*f), count.bind(*f)),
            other => panic!("no bind for {other:?}"),
        };
    }
    let rows = select.fetch_all(&db.pool).await.unwrap();
    let total: i64 = count.fetch_one(&db.pool).await.unwrap().get(0);
    (rows.iter().map(|row| row.get::<i64, _>(0)).collect(), total)
}

#[tokio::test]
async fn a_filter_narrows_in_the_where_and_the_count_agrees() {
    let Some(db) = database().await else {
        return;
    };
    let (rows, total) = ids(&db, Pred::Always, "status=published&score__ge=5").await;
    assert_eq!(rows, [6, 8, 10, 12]);
    assert_eq!(total, 4, "the count reads the same WHERE");
}

#[tokio::test]
async fn a_filter_never_widens_the_rule() {
    let Some(db) = database().await else {
        return;
    };
    // The rule admits author 1 only. Asking for author 2 finds nothing,
    // rather than author 2's rows.
    let rule = Pred::eq(PostColumn::AuthorId, Bound::or_deny(Some(1_i64)));
    let (rows, total) = ids(&db, rule.clone(), "author=2").await;
    assert!(rows.is_empty());
    assert_eq!(total, 0);
    let (rows, _) = ids(&db, rule, "").await;
    assert_eq!(rows, [1, 4, 7, 10]);
    let (rows, _) = ids(&db, Pred::Never, "status=draft").await;
    assert!(rows.is_empty(), "a rule of Never stays Never");
}

#[tokio::test]
async fn a_repeated_key_is_in_and_a_sort_orders_with_the_key_last() {
    let Some(db) = database().await else {
        return;
    };
    let (rows, _) = ids(&db, Pred::Always, "author=0&author=2&sort=-score").await;
    assert_eq!(rows, [12, 11, 9, 8, 6, 5, 3, 2]);
    let (rows, _) = ids(&db, Pred::Always, "author__in=1,2&featured=true").await;
    assert!(rows.is_empty(), "featured posts have author 0");
    let (rows, _) = ids(&db, Pred::Always, "featured=yes").await;
    assert_eq!(rows, [3, 6, 9, 12]);
}

#[tokio::test]
async fn null_and_not_equal_keep_their_meaning() {
    let Some(db) = database().await else {
        return;
    };
    let (rows, _) = ids(&db, Pred::Always, "closed_at__null=false").await;
    assert_eq!(rows, [4, 8, 12]);
    let (rows, total) = ids(&db, Pred::Always, "closed_at__null=true&status__ne=draft").await;
    assert_eq!(rows, [2, 6, 10]);
    assert_eq!(total, 3);
}

#[tokio::test]
async fn an_empty_value_and_an_undeclared_name_are_no_filter() {
    let Some(db) = database().await else {
        return;
    };
    let (rows, _) = ids(&db, Pred::Always, "status=&page=2&q=hello&sort=").await;
    assert_eq!(rows.len(), 12);
}

#[test]
fn a_refused_query_says_what_to_do() {
    let filters = declared();
    let cases = [
        ("status=archived", "one of draft, published"),
        (
            "status__gt=draft",
            "use one of status, status__ne, status__in",
        ),
        ("score=high", "a whole number"),
        ("featured=maybe", "true or false"),
        ("closed_at=2026", "use one of closed_at__null"),
        ("sort=title", "use one of score"),
        ("author__in=1,x", "a whole number"),
    ];
    for (query, needle) in cases {
        let error = filters.parse(query).expect_err(query).to_string();
        assert!(error.contains(needle), "{query}: {error}");
        assert!(error.contains("help:"), "{query}: {error}");
    }
    assert!(matches!(
        filters.parse("sort=nope"),
        Err(FilterError::Sort { .. })
    ));
}

#[test]
fn the_applied_filters_fill_a_form_back_in() {
    let applied = declared()
        .parse("status=draft&status=published&score__ge=3&sort=-score")
        .unwrap();
    assert!(applied.is("status", "draft"));
    assert!(applied.is("status", "published"));
    assert_eq!(applied.value("score", Lookup::Ge), Some("3"));
    assert_eq!(applied.sort_param().as_deref(), Some("-score"));
    assert!(!applied.is_empty());
    let none = declared().parse("").unwrap();
    assert!(none.is_empty());
    assert_eq!(none.pred(), &Pred::Always);
    assert_eq!(none.sort_param(), None, "the default sort stays implicit");
}

/// `status` as free text, so a value the client writes reaches the bind.
fn free_text() -> Filters<PostColumn> {
    Filters::new()
        .text("status", PostColumn::Status)
        .float("score", PostColumn::Score)
}

async fn rows_for(
    db: &Db,
    filters: &Filters<PostColumn>,
    rule: Pred<PostColumn>,
    query: &str,
) -> Vec<i64> {
    let condition = filters
        .parse(query)
        .expect("accepted")
        .condition::<Post>(rule, Dialect::Postgres)
        .expect("compiles");
    let sql = format!(
        "SELECT \"posts\".\"id\" FROM \"posts\" WHERE {} ORDER BY \"posts\".\"id\"",
        condition.sql()
    );
    let mut select = sqlx::query(AssertSqlSafe(sql));
    for value in condition.binds() {
        select = match value {
            Value::Int(n) => select.bind(*n),
            Value::Text(t) => select.bind(t.clone()),
            Value::Bool(b) => select.bind(*b),
            Value::Float(f) => select.bind(*f),
            other => panic!("no bind for {other:?}"),
        };
    }
    let rows = select.fetch_all(&db.pool).await.unwrap();
    rows.iter().map(|row| row.get::<i64, _>(0)).collect()
}

#[tokio::test]
async fn an_injection_looking_value_is_a_bind_and_matches_nothing() {
    let Some(db) = database().await else {
        return;
    };
    let filters = free_text();
    for value in [
        "draft' OR '1'='1",
        "draft\" OR 1=1 --",
        "x'); DROP TABLE posts; --",
        "\\' OR 1=1 /*",
        "draft%' OR status LIKE '%",
    ] {
        let query = format!("status={}", brocanth_pagination::query::encode(value));
        let applied = filters.parse(&query).unwrap();
        let condition = applied
            .condition::<Post>(Pred::Always, Dialect::Postgres)
            .unwrap();
        assert!(!condition.sql().contains(value), "{value} is not SQL text");
        assert_eq!(condition.binds(), [Value::Text(value.into())]);
        assert!(rows_for(&db, &filters, Pred::Always, &query)
            .await
            .is_empty());
    }
    // The table is still there, with every row.
    assert_eq!(rows_for(&db, &filters, Pred::Always, "").await.len(), 12);
    // `__in` splits on `,`, and each part is its own bind.
    let applied = filters.parse("status__in=draft,x'%20OR%201=1").unwrap();
    let condition = applied
        .condition::<Post>(Pred::Always, Dialect::Postgres)
        .unwrap();
    assert_eq!(
        condition.binds(),
        [Value::Text("draft".into()), Value::Text("x' OR 1=1".into())]
    );
}

#[test]
fn an_injection_in_a_typed_filter_or_a_sort_is_refused() {
    let filters = declared();
    for query in [
        "author=1%20OR%201=1",
        "author=1;DROP%20TABLE%20posts",
        "score__in=1,2)%20OR%20(1=1",
        "status=draft'--",
        "featured=true%20OR%201",
        "sort=score;DROP%20TABLE%20posts",
        "sort=score%20DESC",
        "sort=\"score\"",
    ] {
        let error = filters.parse(query).expect_err(query).to_string();
        assert!(error.contains("help:"), "{query}: {error}");
    }
}

#[test]
fn an_unknown_name_is_left_alone_and_an_unknown_lookup_is_refused() {
    let filters = declared();
    let applied = filters
        .parse("nope=1&__ne=x&Status=draft&q=x&per_page=5")
        .unwrap();
    assert!(applied.is_empty(), "names match exactly, case included");
    for query in [
        "status__bogus=draft",
        "status__ne__in=draft",
        "score__like=1",
        "featured__ne=true",
        "author__null=true",
    ] {
        let error = filters.parse(query).expect_err(query);
        assert!(matches!(error, FilterError::Lookup { .. }), "{query}");
        assert!(error.to_string().contains("help:"), "{query}");
    }
    // `__eq` is `=`, written out.
    assert!(filters
        .parse("status__eq=draft")
        .unwrap()
        .is("status", "draft"));
}

#[test]
fn a_value_of_the_wrong_type_is_refused() {
    let declared = declared();
    for query in [
        "score=1.5",
        "score=9223372036854775808",
        "score=0x10",
        "score=1e3",
    ] {
        let error = declared.parse(query).expect_err(query);
        assert!(matches!(error, FilterError::Value { .. }), "{query}");
    }
    let floats = free_text();
    for query in ["score=NaN", "score=inf", "score=-infinity", "score=1e400"] {
        let error = floats.parse(query).expect_err(query).to_string();
        assert!(error.contains("not a number"), "{query}: {error}");
    }
    // Whitespace around a number and the case of a boolean are forgiven.
    assert!(declared.parse("score=%205%20&featured=TRUE").is_ok());
    assert!(declared.parse("closed_at__null=maybe").is_err());
    // A choice is exact: no case folding, no padding.
    for query in ["status=Draft", "status=draft%20", "status=draft%00"] {
        assert!(declared.parse(query).is_err(), "{query}");
    }
}

#[tokio::test]
async fn a_repeated_comparison_is_anded_and_a_repeated_sort_is_checked() {
    let Some(db) = database().await else {
        return;
    };
    let (rows, _) = ids(&db, Pred::Always, "score__gt=2&score__gt=8&score__lt=11").await;
    assert_eq!(rows, [10, 11], "every comparison holds");
    let (rows, _) = ids(&db, Pred::Always, "status=draft&status=draft").await;
    assert_eq!(rows, [1, 3, 5, 7, 9, 11]);
    assert!(declared().parse("sort=score&sort=nope").is_err());
}

#[test]
fn a_sort_on_a_column_that_is_not_declared_is_refused() {
    let filters = declared();
    // `id` is the default sort, but not one a request may ask for.
    for query in [
        "sort=id",
        "sort=-id",
        "sort=-",
        "sort=--score",
        "sort=SCORE",
    ] {
        let error = filters.parse(query).expect_err(query);
        assert!(matches!(error, FilterError::Sort { .. }), "{query}");
        assert!(error.to_string().contains("help:"), "{query}");
    }
    let applied = filters.parse("sort=%2Bscore").unwrap();
    assert_eq!(applied.sort().unwrap().direction, Direction::Ascending);
    let error = Filters::<PostColumn>::new()
        .parse("sort=score")
        .unwrap_err()
        .to_string();
    assert!(error.contains("this list has no sort"), "{error}");
}

#[tokio::test]
async fn a_rule_with_an_or_stays_whole_when_a_filter_is_anded() {
    let Some(db) = database().await else {
        return;
    };
    // Author 1 OR featured. A filter after it must not bind tighter than
    // the rule's OR: `A OR B AND C` would let author 1's drafts through.
    let rule = Pred::any([
        Pred::eq(PostColumn::AuthorId, 1_i64),
        Pred::eq(PostColumn::Featured, true),
    ]);
    let expected: Vec<i64> = (1..=12_i64)
        .filter(|id| (id % 3 == 1 || id % 3 == 0) && id % 2 == 0)
        .collect();
    let (rows, total) = ids(&db, rule.clone(), "status=published").await;
    assert_eq!(rows, expected);
    assert_eq!(total, i64::try_from(expected.len()).unwrap());

    // A filter that names rows outside the rule finds only the overlap.
    let (rows, _) = ids(&db, rule.clone(), "author=2").await;
    assert!(rows.is_empty(), "author 2 has no featured post");
    let (rows, _) = ids(&db, rule.clone(), "author__ne=1").await;
    assert_eq!(rows, [3, 6, 9, 12], "only the featured half of the rule");
    // A `__ne` never inverts the rule.
    let (rows, _) = ids(&db, rule, "featured=false").await;
    assert_eq!(rows, [1, 4, 7, 10]);
}

#[tokio::test]
async fn an_in_list_of_only_commas_matches_nothing_and_widens_nothing() {
    let Some(db) = database().await else {
        return;
    };
    let applied = declared().parse("author__in=,,").unwrap();
    assert_eq!(applied.pred(), &Pred::Never);
    let (rows, total) = ids(&db, Pred::Always, "author__in=,,").await;
    assert!(rows.is_empty());
    assert_eq!(total, 0);
}

#[test]
fn a_filtered_list_keeps_its_filters_through_a_page_link() {
    use brocanth_pagination::{Links, Params};
    let query = "status=draft&score__ge=3&sort=-score&page=2&q=caf%C3%A9";
    let filters = declared();
    let before = filters.parse(query).unwrap();
    let link = Links::new("/posts", query).page(3);
    let (_, next) = link.split_once('?').unwrap();
    assert_eq!(Params::parse(next).unwrap().page, 3);
    let after = filters.parse(next).unwrap();
    assert_eq!(after.pred(), before.pred());
    assert_eq!(after.sort_param(), before.sort_param());
    assert_eq!(after.active(), before.active());
}
