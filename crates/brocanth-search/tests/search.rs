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
fn a_control_character_is_sent_as_a_space() {
    let query = documents::index()
        .query(&condition(Pred::Always), "a\0b\u{7}c\nd", 0, 20)
        .unwrap();
    assert_eq!(
        query.select.binds.last(),
        Some(&Value::Text("a b c d".into()))
    );
}

#[test]
fn a_page_size_past_bigint_is_written_as_the_largest_bigint() {
    let query = documents::index()
        .query(&condition(Pred::Always), "x", u64::MAX, u64::MAX)
        .unwrap();
    let max = i64::MAX;
    assert!(
        query
            .select
            .sql
            .ends_with(&format!("LIMIT {max} OFFSET {max}")),
        "{}",
        query.select.sql
    );
}

#[test]
fn an_empty_index_says_what_to_do() {
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

async fn search(scoped: &Scoped, rule: Pred<DocumentColumn>, term: &str) -> Vec<String> {
    documents::index()
        .search::<Hit>(&scoped.db, &condition(rule), term, 1, 50)
        .await
        .unwrap_or_else(|error| panic!("{term:?}: {error}"))
        .items
        .into_iter()
        .map(|hit| hit.key)
        .collect()
}

#[tokio::test]
async fn punctuation_and_operator_noise_find_nothing_and_do_not_fail() {
    let Some(scoped) = scoped().await else { return };
    fill(&scoped.db).await;
    for term in [
        "!!!",
        "&",
        "|",
        "!",
        "()",
        ":*",
        "'",
        "\"",
        "\"\"",
        "-",
        "--",
        "or",
        "&&&|||!!!",
        "%",
        "\\",
        "<->",
        "'; DROP TABLE search_documents; --",
    ] {
        assert!(
            search(&scoped, Pred::Always, term).await.is_empty(),
            "{term:?}"
        );
    }
    // The table is still there.
    assert_eq!(search(&scoped, Pred::Always, "tomatoes").await, ["3"]);
    scoped.drop_schema().await;
}

#[tokio::test]
async fn websearch_syntax_reads_phrases_negation_and_or() {
    let Some(scoped) = scoped().await else { return };
    fill(&scoped.db).await;
    let mut keys = search(&scoped, Pred::Always, "postgres -pasta").await;
    keys.sort();
    assert_eq!(keys, ["1", "about"]);
    assert_eq!(search(&scoped, Pred::Always, "\"boil water\"").await, ["2"]);
    let mut keys = search(&scoped, Pred::Always, "tomatoes or ranking").await;
    keys.sort();
    assert_eq!(keys, ["1", "3"]);
    // An unclosed quote is still a search.
    assert_eq!(search(&scoped, Pred::Always, "\"basil").await, ["3"]);
    scoped.drop_schema().await;
}

#[tokio::test]
async fn a_unicode_term_finds_its_row() {
    let Some(scoped) = scoped().await else { return };
    fill(&scoped.db).await;
    for (key, title, body) in [
        ("cafe", "Le café", "crème brûlée à la française"),
        ("jp", "日本語", "東京 タワー"),
        ("emoji", "Party", "🎉 celebration 🎉"),
    ] {
        documents::upsert(&scoped.db, "post", key, title, body, "/x")
            .await
            .unwrap();
    }
    assert_eq!(search(&scoped, Pred::Always, "café").await, ["cafe"]);
    assert_eq!(search(&scoped, Pred::Always, "brûlée").await, ["cafe"]);
    assert_eq!(search(&scoped, Pred::Always, "CRÈME").await, ["cafe"]);
    assert_eq!(search(&scoped, Pred::Always, "日本語").await, ["jp"]);
    assert_eq!(
        search(&scoped, Pred::Always, "celebration 🎉").await,
        ["emoji"]
    );
    assert!(search(&scoped, Pred::Always, "🎉🎉🎉").await.is_empty());
    scoped.drop_schema().await;
}

#[tokio::test]
async fn a_control_character_in_the_term_is_not_a_database_error() {
    let Some(scoped) = scoped().await else { return };
    fill(&scoped.db).await;
    // `?q=%00tomatoes` decodes to a NUL byte, which Postgres refuses in a
    // text value.
    assert_eq!(search(&scoped, Pred::Always, "toma\0toes").await.len(), 0);
    assert_eq!(search(&scoped, Pred::Always, "\0tomatoes").await, ["3"]);
    assert_eq!(
        search(&scoped, Pred::Always, "tomatoes\u{7}\t\n").await,
        ["3"]
    );
    scoped.drop_schema().await;
}

#[tokio::test]
async fn a_very_long_term_is_cut_and_still_searched() {
    let Some(scoped) = scoped().await else { return };
    fill(&scoped.db).await;
    // Every word must match, so a pasted page finds nothing, but it is
    // still a search and not a database error.
    let long = format!("tomatoes {}", "ü日🎉 ".repeat(10_000));
    assert!(search(&scoped, Pred::Always, &long).await.is_empty());
    let words = format!("tomatoes {}", "basil ".repeat(5_000));
    assert_eq!(search(&scoped, Pred::Always, &words).await, ["3"]);
    let one_word = "a".repeat(100_000);
    assert!(search(&scoped, Pred::Always, &one_word).await.is_empty());
    scoped.drop_schema().await;
}

#[tokio::test]
async fn a_title_outranks_a_body_and_ties_go_by_id() {
    let Some(scoped) = scoped().await else { return };
    for (key, title, body) in [
        ("body-once", "Notes", "one mention of zebra here"),
        ("title", "Zebra", "nothing else"),
        ("body-twice", "More notes", "zebra and zebra again"),
        ("tie-a", "Zebra", "nothing else"),
    ] {
        documents::upsert(&scoped.db, "post", key, title, body, "/x")
            .await
            .unwrap();
    }
    let hits = documents::index()
        .search::<Hit>(&scoped.db, &condition(Pred::Always), "zebra", 1, 20)
        .await
        .unwrap();
    let keys: Vec<&str> = hits.items.iter().map(|h| h.key.as_str()).collect();
    assert_eq!(keys, ["title", "tie-a", "body-twice", "body-once"]);
    assert!(hits
        .items
        .windows(2)
        .all(|pair| pair[0].search_rank >= pair[1].search_rank));
    assert!(hits.items[0].id < hits.items[1].id, "equal ranks by id");
    scoped.drop_schema().await;
}

#[tokio::test]
async fn the_rule_limits_every_page_and_the_total() {
    let Some(scoped) = scoped().await else { return };
    for n in 1..=25 {
        let kind = if n % 5 == 0 { "secret" } else { "post" };
        documents::upsert(
            &scoped.db,
            kind,
            &n.to_string(),
            &format!("Report {n}"),
            "quarterly report",
            "/x",
        )
        .await
        .unwrap();
    }
    let rule = Pred::is_not(DocumentColumn::Kind, "secret");
    let index = documents::index();
    let mut seen = Vec::new();
    for number in 1..=3 {
        let hits = index
            .search::<Hit>(&scoped.db, &condition(rule.clone()), "report", number, 8)
            .await
            .unwrap();
        assert_eq!(hits.page.total(), 20, "the count reads the rule");
        assert_eq!(hits.page.total_pages(), 3);
        assert!(hits.items.iter().all(|h| h.kind == "post"));
        seen.extend(hits.items.into_iter().map(|h| h.id));
    }
    let mut unique = seen.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), 20, "every allowed row once, none twice");
    assert_eq!(seen.len(), 20);

    // Past the end is the last page; page zero is the first.
    let past = index
        .search::<Hit>(&scoped.db, &condition(rule.clone()), "report", 999, 8)
        .await
        .unwrap();
    assert_eq!(past.page.number(), 3);
    assert_eq!(past.items.len(), 4);
    let zero = index
        .search::<Hit>(&scoped.db, &condition(rule.clone()), "report", 0, 8)
        .await
        .unwrap();
    assert_eq!(zero.page.number(), 1);
    assert_eq!(zero.items.len(), 8);
    // A page size of zero is one, and a huge one is still a statement.
    let one = index
        .search::<Hit>(&scoped.db, &condition(rule.clone()), "report", 1, 0)
        .await
        .unwrap();
    assert_eq!(one.items.len(), 1);
    let huge = index
        .search::<Hit>(&scoped.db, &condition(rule), "report", 1, u64::MAX)
        .await
        .unwrap();
    assert_eq!(huge.items.len(), 20);
    scoped.drop_schema().await;
}
