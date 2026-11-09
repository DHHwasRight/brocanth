//! The SQL a search writes, and, with `BROCANTH_TEST_DATABASE_URL`, what
//! Postgres answers to it: ranked rows, a typo found by trigram, and a rule
//! that narrows the hits and the count alike.
//!
//! Each Postgres test runs in a schema of its own and drops it after.

use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use brocanth_db::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use brocanth_db::sqlx::{self, AssertSqlSafe, PgPool};
use brocanth_db::Db;
use brocanth_policy::{Dialect, Pred, Value};
use brocanth_search::documents::{self, Document, DocumentColumn, Hit};
use brocanth_search::{SearchError, MAX_TERM};

fn condition(rule: Pred<DocumentColumn>) -> brocanth_policy::Condition {
    rule.condition::<Document>(Dialect::Postgres)
        .expect("compiles")
}

#[test]
fn the_rule_comes_first_and_the_term_is_a_bind() {
    let rule = Pred::eq(DocumentColumn::Kind, "post");
    let query = documents::index()
        .query(&condition(rule), "rust'; DROP TABLE x; --", 40, 20)
        .unwrap();
    assert!(
        query.count.sql.starts_with(
            "SELECT count(*) FROM \"search_documents\" WHERE (\"search_documents\".\"kind\" = $1) AND ("
        ),
        "{}",
        query.count.sql
    );
    assert!(
        !query.select.sql.contains("DROP TABLE"),
        "the term is never SQL text"
    );
    assert!(query
        .select
        .sql
        .contains("websearch_to_tsquery('english', $2)"));
    assert!(query
        .select
        .sql
        .contains("\"search_documents\".\"title\" % $2"));
    assert!(query.select.sql.ends_with(
        "ORDER BY search_rank DESC, \"search_documents\".\"id\" ASC LIMIT 20 OFFSET 40"
    ));
    assert_eq!(
        query.select.binds,
        vec![
            Value::Text("post".into()),
            Value::Text("rust'; DROP TABLE x; --".into())
        ]
    );
    assert_eq!(query.select.binds, query.count.binds, "one WHERE for both");
}

#[test]
fn a_long_term_is_cut_and_not_refused() {
    let term = "word ".repeat(100);
    let query = documents::index()
        .query(&condition(Pred::Always), &term, 0, 20)
        .unwrap();
    let Some(Value::Text(sent)) = query.select.binds.last() else {
        panic!("the term is the last bind");
    };
    assert_eq!(sent.chars().count(), MAX_TERM);
}

#[test]
fn a_sqlite_condition_and_an_empty_index_say_what_to_do() {
    let sqlite = Pred::<DocumentColumn>::Always
        .condition::<Document>(Dialect::Sqlite)
        .unwrap();
    let error = documents::index().query(&sqlite, "x", 0, 20).unwrap_err();
    assert!(matches!(error, SearchError::Dialect));
    assert!(error.to_string().contains("help:"));

    let empty = brocanth_search::Index::<Document>::new("search");
    let error = empty.migration().unwrap_err();
    assert!(matches!(error, SearchError::NoSource { .. }));
    assert!(error.to_string().contains("help:"));
}

#[test]
fn an_excerpt_cuts_at_a_word() {
    let hit = Hit {
        id: 1,
        kind: "post".into(),
        key: "1".into(),
        title: "t".into(),
        body: "the quick brown fox jumps".into(),
        url: "/p/1".into(),
        search_rank: 0.0,
    };
    assert_eq!(hit.excerpt(12), "the quick…");
    assert_eq!(hit.excerpt(100), "the quick brown fox jumps");
}

/// A schema of its own, dropped when the test ends.
struct Scoped {
    db: Db,
    admin: PgPool,
    schema: String,
}

impl Scoped {
    async fn drop_schema(self) {
        self.db.close().await;
        sqlx::query(AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {} CASCADE",
            self.schema
        )))
        .execute(&self.admin)
        .await
        .expect("dropping the test schema");
        self.admin.close().await;
    }
}

async fn scoped() -> Option<Scoped> {
    let Ok(url) = std::env::var("BROCANTH_TEST_DATABASE_URL") else {
        eprintln!("skipped: set BROCANTH_TEST_DATABASE_URL to run the Postgres tests");
        return None;
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let schema = format!(
        "brocanth_search_t{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connecting to the test database");
    // `pg_trgm` lives in `public`, once per database: an extension created
    // inside a test schema would go with that schema, and the migration's
    // `IF NOT EXISTS` would then skip it in the next one. Two tests that
    // create it at once collide on the catalog, so a lock takes turns.
    sqlx::raw_sql(AssertSqlSafe(
        "BEGIN; SELECT pg_advisory_xact_lock(7340301); \
         CREATE EXTENSION IF NOT EXISTS pg_trgm SCHEMA public; COMMIT;",
    ))
    .execute(&admin)
    .await
    .expect("creating pg_trgm");
    for statement in [
        format!("DROP SCHEMA IF EXISTS {schema} CASCADE"),
        format!("CREATE SCHEMA {schema}"),
    ] {
        sqlx::query(AssertSqlSafe(statement))
            .execute(&admin)
            .await
            .expect("making the test schema");
    }
    let path = format!("{schema},public");
    let options = PgConnectOptions::from_str(&url)
        .expect("a Postgres URL")
        .options([("search_path", path.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("connecting to the test schema");
    sqlx::raw_sql(AssertSqlSafe(documents::migration().up))
        .execute(&pool)
        .await
        .expect("the site index migration applies");
    Some(Scoped {
        db: Db::from_pool(pool),
        admin,
        schema,
    })
}

async fn fill(db: &Db) {
    let rows = [
        (
            "post",
            "1",
            "Postgres full-text search",
            "tsvector and ranking in one database",
        ),
        (
            "post",
            "2",
            "Cooking pasta",
            "boil water; postgres is not involved",
        ),
        (
            "page",
            "about",
            "About us",
            "a small team that likes postgres",
        ),
        ("post", "3", "Gardening", "tomatoes and basil"),
    ];
    for (kind, key, title, body) in rows {
        documents::upsert(db, kind, key, title, body, &format!("/{kind}/{key}"))
            .await
            .expect("upsert");
    }
}

#[tokio::test]
async fn a_title_match_ranks_first_and_the_count_agrees() {
    let Some(scoped) = scoped().await else { return };
    fill(&scoped.db).await;
    let hits = documents::index()
        .search::<Hit>(&scoped.db, &condition(Pred::Always), "postgres", 1, 2)
        .await
        .expect("search");
    assert_eq!(hits.page.total(), 3, "every row that says postgres");
    assert_eq!(hits.items.len(), 2, "one page of two");
    assert_eq!(hits.items[0].key, "1", "the title match ranks first");

    let second = documents::index()
        .search::<Hit>(&scoped.db, &condition(Pred::Always), "postgres", 2, 2)
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(
        hits.items.iter().all(|a| a.id != second.items[0].id),
        "two pages never share a row"
    );
    scoped.drop_schema().await;
}

#[tokio::test]
async fn a_typo_in_a_title_finds_it_by_trigram() {
    let Some(scoped) = scoped().await else { return };
    fill(&scoped.db).await;
    let hits = documents::index()
        .search::<Hit>(&scoped.db, &condition(Pred::Always), "Gardenning", 1, 20)
        .await
        .unwrap();
    assert_eq!(
        hits.items
            .iter()
            .map(|h| h.key.as_str())
            .collect::<Vec<_>>(),
        ["3"]
    );
    scoped.drop_schema().await;
}

#[tokio::test]
async fn the_rule_narrows_the_hits_and_the_total() {
    let Some(scoped) = scoped().await else { return };
    fill(&scoped.db).await;
    let rule = Pred::eq(DocumentColumn::Kind, "page");
    let hits = documents::index()
        .search::<Hit>(&scoped.db, &condition(rule), "postgres", 1, 20)
        .await
        .unwrap();
    assert_eq!(hits.page.total(), 1);
    assert_eq!(hits.items[0].key, "about");

    let none = documents::index()
        .search::<Hit>(&scoped.db, &condition(Pred::Never), "postgres", 1, 20)
        .await
        .unwrap();
    assert!(none.items.is_empty());
    assert_eq!(none.page.total(), 0);

    assert_eq!(
        documents::remove(&scoped.db, "page", "about")
            .await
            .unwrap(),
        1
    );
    let empty = documents::index()
        .search::<Hit>(&scoped.db, &condition(Pred::Always), "   ", 1, 20)
        .await
        .unwrap();
    assert!(empty.items.is_empty(), "an empty term finds nothing");
    scoped.drop_schema().await;
}
