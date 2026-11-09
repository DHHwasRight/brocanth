//! Filters narrow the rows in the database, beside the rule, and a query
//! string the declaration refuses is an error with a `help:` line.

use brocanth_filters::{Direction, FilterError, Filters, Lookup};
use brocanth_policy::{Bound, Column, Dialect, Entity, Ident, Pred, Value};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{AssertSqlSafe, Row, SqlitePool};

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

/// Twelve posts: three authors, two statuses, scores 0 to 11, every third
/// one featured, every fourth one closed.
async fn database() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("sqlite in memory");
    sqlx::query(
        "CREATE TABLE posts (id INTEGER PRIMARY KEY, author_id INTEGER, status TEXT, \
         score INTEGER, featured BOOLEAN, closed_at TEXT)",
    )
    .execute(&pool)
    .await
    .unwrap();
    for id in 1..=12_i64 {
        sqlx::query("INSERT INTO posts VALUES (?, ?, ?, ?, ?, ?)")
            .bind(id)
            .bind(id % 3)
            .bind(if id % 2 == 0 { "published" } else { "draft" })
            .bind(id - 1)
            .bind(id % 3 == 0)
            .bind((id % 4 == 0).then_some("2026-11-09"))
            .execute(&pool)
            .await
            .unwrap();
    }
    pool
}

/// The ids the database returns for the rule AND the filters, in the
/// order the filters ask for, and the count of the same `WHERE`.
async fn ids(pool: &SqlitePool, rule: Pred<PostColumn>, query: &str) -> (Vec<i64>, i64) {
    let applied = declared().parse(query).expect("a query the list accepts");
    let condition = applied
        .condition::<Post>(rule, Dialect::Sqlite)
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
    let rows = select.fetch_all(pool).await.unwrap();
    let total: i64 = count.fetch_one(pool).await.unwrap().get(0);
    (rows.iter().map(|row| row.get::<i64, _>(0)).collect(), total)
}

#[tokio::test]
async fn a_filter_narrows_in_the_where_and_the_count_agrees() {
    let pool = database().await;
    let (rows, total) = ids(&pool, Pred::Always, "status=published&score__ge=5").await;
    assert_eq!(rows, [6, 8, 10, 12]);
    assert_eq!(total, 4, "the count reads the same WHERE");
}

#[tokio::test]
async fn a_filter_never_widens_the_rule() {
    let pool = database().await;
    // The rule admits author 1 only. Asking for author 2 finds nothing,
    // rather than author 2's rows.
    let rule = Pred::eq(PostColumn::AuthorId, Bound::or_deny(Some(1_i64)));
    let (rows, total) = ids(&pool, rule.clone(), "author=2").await;
    assert!(rows.is_empty());
    assert_eq!(total, 0);
    let (rows, _) = ids(&pool, rule, "").await;
    assert_eq!(rows, [1, 4, 7, 10]);
    let (rows, _) = ids(&pool, Pred::Never, "status=draft").await;
    assert!(rows.is_empty(), "a rule of Never stays Never");
}

#[tokio::test]
async fn a_repeated_key_is_in_and_a_sort_orders_with_the_key_last() {
    let pool = database().await;
    let (rows, _) = ids(&pool, Pred::Always, "author=0&author=2&sort=-score").await;
    assert_eq!(rows, [12, 11, 9, 8, 6, 5, 3, 2]);
    let (rows, _) = ids(&pool, Pred::Always, "author__in=1,2&featured=true").await;
    assert!(rows.is_empty(), "featured posts have author 0");
    let (rows, _) = ids(&pool, Pred::Always, "featured=yes").await;
    assert_eq!(rows, [3, 6, 9, 12]);
}

#[tokio::test]
async fn null_and_not_equal_keep_their_meaning() {
    let pool = database().await;
    let (rows, _) = ids(&pool, Pred::Always, "closed_at__null=false").await;
    assert_eq!(rows, [4, 8, 12]);
    let (rows, total) = ids(&pool, Pred::Always, "closed_at__null=true&status__ne=draft").await;
    assert_eq!(rows, [2, 6, 10]);
    assert_eq!(total, 3);
}

#[tokio::test]
async fn an_empty_value_and_an_undeclared_name_are_no_filter() {
    let pool = database().await;
    let (rows, _) = ids(&pool, Pred::Always, "status=&page=2&q=hello&sort=").await;
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
