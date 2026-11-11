//! A keyset `WHERE` on a real PostgreSQL. Each test makes its own schema and
//! drops it at the end.
//!
//! ```sh
//! BROCANTH_TEST_DATABASE_URL=postgres://postgres:pw@localhost:55432/postgres \
//!   cargo test -p brocanth-pagination --test postgres
//! ```
//!
//! Without a URL, each test says `!! SKIPPED` on the real stderr and passes.

use std::sync::atomic::{AtomicU32, Ordering};

use brocanth_db::sqlx::{self, AssertSqlSafe, Row as _};
use brocanth_db::{Db, DbConfig};
use brocanth_pagination::{Cursor, Direction, Key, Keyset, KeysetPage, Params};
use brocanth_policy::{Column, Dialect, Entity, Ident, Pred, SortKind, Sortable, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PostColumn {
    Id,
    Score,
}

impl Column for PostColumn {
    const ALL: &'static [Self] = &[Self::Id, Self::Score];

    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Score => "score",
        }
    }
}

impl Sortable for PostColumn {
    fn sort_kind(&self) -> Option<SortKind> {
        Some(SortKind::Int)
    }
}

struct Post;

impl Entity for Post {
    type Column = PostColumn;
    const TABLE: Ident = Ident::must("posts");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}

fn url() -> Option<String> {
    ["BROCANTH_TEST_DATABASE_URL"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|u| !u.trim().is_empty()))
}

fn skipped() {
    use std::io::Write as _;
    let name = std::thread::current().name().unwrap_or("a test").to_owned();
    let _ = std::io::stderr()
        .write_all(format!("!! SKIPPED {name}: set BROCANTH_TEST_DATABASE_URL\n").as_bytes());
}

async fn exec(db: &Db, sql: &str) {
    sqlx::query(AssertSqlSafe(sql.to_owned()))
        .execute(db.pool())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// A schema of its own with `posts(id, score)`: ids 1 to 6, scores
/// 1, 2, 2, 2, 3, 3.
async fn schema(url: &str) -> (Db, Db, String) {
    static N: AtomicU32 = AtomicU32::new(0);
    let schema = format!(
        "brocanth_pagination_t{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let admin = Db::connect(&DbConfig::new(url)).await.expect("connect");
    exec(&admin, &format!("CREATE SCHEMA {schema}")).await;
    let mut cfg = DbConfig::new(url);
    cfg.search_path = vec![schema.clone()];
    let db = Db::connect(&cfg).await.expect("connect");
    exec(
        &db,
        "CREATE TABLE posts (id BIGINT PRIMARY KEY, score BIGINT NOT NULL)",
    )
    .await;
    exec(
        &db,
        "INSERT INTO posts VALUES (1, 1), (2, 2), (3, 2), (4, 2), (5, 3), (6, 3)",
    )
    .await;
    (admin, db, schema)
}

/// One page: the rows `pred` admits, in the keyset's order, at most `limit`.
async fn read(
    db: &Db,
    keyset: &Keyset<PostColumn>,
    pred: Pred<PostColumn>,
    limit: u64,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    let condition = pred.condition::<Post>(Dialect::Postgres).expect("compiles");
    let sql = format!(
        "SELECT id, score FROM posts WHERE {} ORDER BY {} LIMIT {limit}",
        condition.sql(),
        keyset.order_by::<Post>().expect("order"),
    );
    let mut query = sqlx::query(AssertSqlSafe(sql));
    for value in condition.binds() {
        query = match value {
            Value::Int(n) => query.bind(*n),
            Value::Text(text) => query.bind(text.clone()),
            other => panic!("no bind for {other:?} in this test"),
        };
    }
    let rows = query.fetch_all(db.pool()).await?;
    Ok(rows
        .iter()
        .map(|row| (row.get::<i64, _>("id"), row.get::<i64, _>("score")))
        .collect())
}

#[tokio::test]
async fn a_cursor_from_the_query_string_reads_the_rows_after_it() {
    let Some(url) = url() else {
        skipped();
        return;
    };
    let (admin, db, schema) = schema(&url).await;
    let keyset = Keyset::new(PostColumn::Score, PostColumn::Id, Direction::Ascending);
    let params = Params::parse("after=i2.i3").unwrap();
    let pred = keyset.after(params.after.as_ref()).unwrap();
    let rows = read(&db, &keyset, pred, 10).await.expect("the query runs");
    assert_eq!(rows, [(4, 2), (5, 3), (6, 3)]);

    exec(&admin, &format!("DROP SCHEMA {schema} CASCADE")).await;
}

#[tokio::test]
async fn a_previous_link_reads_back_the_page_before_through_a_tie() {
    let Some(url) = url() else {
        skipped();
        return;
    };
    let (admin, db, schema) = schema(&url).await;
    let keyset = Keyset::new(PostColumn::Score, PostColumn::Id, Direction::Ascending);
    let key = |row: &(i64, i64)| Cursor::new([Key::Int(row.1), Key::Int(row.0)]);
    let fetch = Keyset::<PostColumn>::fetch(2);

    // Forward: [1, 2], [3, 4], [5, 6]. The second page ends inside the tie
    // on score 2.
    let first = read(&db, &keyset, keyset.after(None).unwrap(), fetch)
        .await
        .unwrap();
    let first = KeysetPage::from_rows(first, 2, key);
    let next = first.next.clone().unwrap();
    let rows = read(&db, &keyset, keyset.after(Some(&next)).unwrap(), fetch)
        .await
        .unwrap();
    let second = KeysetPage::after(rows, 2, key);
    assert_eq!(second.items, [(3, 2), (4, 2)]);
    let next = second.next.clone().unwrap();
    let rows = read(&db, &keyset, keyset.after(Some(&next)).unwrap(), fetch)
        .await
        .unwrap();
    let third = KeysetPage::after(rows, 2, key);
    assert_eq!(
        (third.items.as_slice(), &third.next),
        (&[(5, 3), (6, 3)][..], &None)
    );

    // Back: before the third page's first row, in the reversed order.
    let reversed = keyset.reversed();
    let previous = third.previous.clone().unwrap();
    let rows = read(&db, &reversed, keyset.before(&previous).unwrap(), fetch)
        .await
        .unwrap();
    let back = KeysetPage::before(rows, 2, key);
    assert_eq!(back, second);
    let rows = read(
        &db,
        &reversed,
        keyset.before(back.previous.as_ref().unwrap()).unwrap(),
        fetch,
    )
    .await
    .unwrap();
    let home = KeysetPage::before(rows, 2, key);
    assert_eq!(home.items, first.items);
    assert_eq!(home.previous, None, "the first page, reached backwards");

    exec(&admin, &format!("DROP SCHEMA {schema} CASCADE")).await;
}

#[tokio::test]
async fn a_text_cursor_on_an_integer_sort_is_a_400_and_not_a_database_error() {
    let Some(url) = url() else {
        skipped();
        return;
    };
    let (admin, db, schema) = schema(&url).await;
    let keyset = Keyset::new(PostColumn::Score, PostColumn::Id, Direction::Ascending);

    // The hazard is real: text compared with a BIGINT column is an error on
    // PostgreSQL, which a handler can only answer with a 500.
    let unchecked = Pred::gt(PostColumn::Score, "a");
    let error = read(&db, &keyset, unchecked, 10)
        .await
        .expect_err("PostgreSQL refuses bigint > text");
    assert!(matches!(error, sqlx::Error::Database(_)), "{error}");

    // The keyset stops each mistyped cursor before the query.
    for token in ["t61.i3", "i2.t61", "t61.t62"] {
        let params = Params::parse(&format!("after={token}")).unwrap();
        let cursor = params.after.expect("it parses as a token");
        let error = keyset
            .after(Some(&cursor))
            .expect_err("a key of the wrong kind");
        let message = error.to_string();
        assert!(message.contains("is not a cursor"), "{token}: {message}");
        assert!(message.contains("help:"), "{token}: {message}");
        assert_eq!(error.status(), 400, "{token}");
    }

    exec(&admin, &format!("DROP SCHEMA {schema} CASCADE")).await;
}
