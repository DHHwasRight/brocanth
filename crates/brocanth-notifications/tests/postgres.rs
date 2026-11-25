//! The store against a real Postgres. Each test makes its own schema,
//! points its pool's `search_path` at it, and drops it at the end, so the
//! tests run in parallel and truncate nothing.
//!
//! ```sh
//! BROCANTH_TEST_DATABASE_URL=postgres://postgres:pg@localhost:55510/postgres \
//!   cargo test -p brocanth-notifications
//! ```
//!
//! Without a URL, each test says `!! SKIPPED` on stderr and passes.

use std::sync::atomic::{AtomicU32, Ordering};

use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::{Db, DbConfig};
use brocanth_notifications::{
    Notice, Notification, NotificationColumn, NotificationStore, NotifyError,
};
use brocanth_policy::{Dialect, Pred};

fn url() -> Option<String> {
    [
        "BROCANTH_NOTIFICATIONS_TEST_POSTGRES",
        "BROCANTH_TEST_DATABASE_URL",
    ]
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

/// A store in a schema of its own, migrated, and the admin pool that drops
/// the schema at the end.
struct Scratch {
    store: NotificationStore,
    admin: Db,
    schema: String,
}

impl Scratch {
    async fn new(url: &str) -> Scratch {
        static N: AtomicU32 = AtomicU32::new(0);
        let schema = format!(
            "brocanth_notifications_t{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        );
        let admin = Db::connect(&DbConfig::new(url)).await.expect("connect");
        exec(&admin, &format!("CREATE SCHEMA {schema}")).await;
        let mut cfg = DbConfig::new(url);
        cfg.search_path = vec![schema.clone()];
        let db = Db::connect(&cfg).await.expect("connect");
        let store = NotificationStore::new(db);
        store.migrate().await.expect("the table");
        // Twice: a second replica runs it too.
        store.migrate().await.expect("idempotent");
        Scratch {
            store,
            admin,
            schema,
        }
    }

    async fn drop(self) {
        exec(&self.admin, &format!("DROP SCHEMA {} CASCADE", self.schema)).await;
    }
}

#[tokio::test]
async fn an_inbox_counts_lists_newest_first_and_marks_read() {
    let Some(url) = url() else {
        skipped();
        return;
    };
    let scratch = Scratch::new(&url).await;
    let store = &scratch.store;
    let mut ids = Vec::new();
    for n in 1..=5 {
        let notice = Notice::new("comment", format!("reply {n}")).url(format!("/posts/{n}"));
        ids.push(store.notify("ada", &notice).await.expect("stored"));
    }
    let inbox = store.inbox("ada");
    assert_eq!(inbox.unread().await.unwrap(), 5);

    let first = inbox.list(None, 2, None).await.unwrap();
    let titles: Vec<&str> = first.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["reply 5", "reply 4"]);
    assert_eq!(first[0].url.as_deref(), Some("/posts/5"));
    assert!(first[0].created_at > 1_700_000_000);
    // The next page starts below the last id, so a new row does not shift it.
    store
        .notify("ada", &Notice::new("comment", "reply 6"))
        .await
        .unwrap();
    let next = inbox.list(Some(first[1].id), 2, None).await.unwrap();
    let titles: Vec<&str> = next.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["reply 3", "reply 2"]);

    assert!(inbox.mark_read(ids[0]).await.unwrap());
    assert!(
        inbox.mark_read(ids[0]).await.unwrap(),
        "already read is still held"
    );
    assert_eq!(inbox.unread().await.unwrap(), 5);
    let read = inbox.list(Some(ids[0] + 1), 1, None).await.unwrap();
    assert!(read[0].read);
    assert_eq!(inbox.mark_all_read().await.unwrap(), 5);
    assert_eq!(inbox.unread().await.unwrap(), 0);
    scratch.drop().await;
}

#[tokio::test]
async fn an_account_never_sees_or_marks_another_accounts_notifications() {
    let Some(url) = url() else {
        skipped();
        return;
    };
    let scratch = Scratch::new(&url).await;
    let store = &scratch.store;
    let theirs = store
        .notify("ada", &Notice::new("invoice", "Invoice 12 is due"))
        .await
        .unwrap();
    store
        .notify("bob", &Notice::new("comment", "hello"))
        .await
        .unwrap();

    let bob = store.inbox("bob");
    assert_eq!(bob.unread().await.unwrap(), 1);
    let listed = bob.list(None, 50, None).await.unwrap();
    assert!(listed.iter().all(|item| item.id != theirs), "{listed:?}");
    // A forged id from another account matches nothing.
    assert!(!bob.mark_read(theirs).await.unwrap());
    assert_eq!(bob.mark_all_read().await.unwrap(), 1);
    assert_eq!(store.inbox("ada").unread().await.unwrap(), 1);

    // A policy rule narrows the list; one that names another account finds
    // nothing, because the inbox's account is in the same `WHERE`.
    let other = Pred::eq(NotificationColumn::Account, "ada")
        .condition::<Notification>(Dialect::Postgres)
        .unwrap();
    assert!(bob.list(None, 50, Some(&other)).await.unwrap().is_empty());
    scratch.drop().await;
}

#[tokio::test]
async fn a_policy_rule_narrows_the_inbox_by_its_columns() {
    let Some(url) = url() else {
        skipped();
        return;
    };
    let scratch = Scratch::new(&url).await;
    let store = &scratch.store;
    store
        .notify("ada", &Notice::new("comment", "c1"))
        .await
        .unwrap();
    store
        .notify("ada", &Notice::new("invoice", "i1"))
        .await
        .unwrap();
    store
        .notify("ada", &Notice::new("comment", "c2"))
        .await
        .unwrap();
    let comments = Pred::eq(NotificationColumn::Kind, "comment")
        .condition::<Notification>(Dialect::Postgres)
        .unwrap();
    let listed = store
        .inbox("ada")
        .list(None, 50, Some(&comments))
        .await
        .unwrap();
    let titles: Vec<&str> = listed.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, ["c2", "c1"]);
    scratch.drop().await;
}

#[tokio::test]
async fn an_unsafe_link_or_an_empty_title_is_refused_before_the_insert() {
    let Some(url) = url() else {
        skipped();
        return;
    };
    let scratch = Scratch::new(&url).await;
    let store = &scratch.store;
    let refused = store
        .notify("ada", &Notice::new("x", "t").url("javascript:alert(1)"))
        .await;
    assert!(
        matches!(refused, Err(NotifyError::UnsafeUrl(_))),
        "{refused:?}"
    );
    let refused = store.notify("ada", &Notice::new("x", "  ")).await;
    assert!(matches!(refused, Err(NotifyError::NoTitle)), "{refused:?}");
    assert_eq!(store.inbox("ada").unread().await.unwrap(), 0);
    scratch.drop().await;
}
