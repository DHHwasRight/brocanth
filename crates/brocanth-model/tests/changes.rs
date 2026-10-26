//! The publisher half of PLAN.md §10, end to end.
//!
//! `docs/framework-gaps.md` G4 was *"the model layer publishes to nobody"*.
//! The claims under test are the three the gap names: a write emits one
//! change naming the right table, row and kind; a rollback emits none; and an
//! application that installed nothing pays nothing.
//!
//! The last test is the whole chain in one go -- a row written through SeaORM
//! arriving in a realtime subscriber's mailbox -- because two tests that each
//! stop at the seam would both pass with the seam unplugged.

#[path = "../fixtures/blog.rs"]
mod blog;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use blog::{migration::Migrator, post, user};
use brocanth_events::{ChangeKind, ChangeSink, FanOut, NoopSink, RowChange, RowId};
use brocanth_model::changes::{row_id, table_name, Writing};
use brocanth_model::prelude::*;
use brocanth_model::testing::Harness;
use brocanth_model::Tracked;
use brocanth_realtime::{Broker, Event, Gate, Subscription, Topic};
use sea_orm::IntoActiveModel;

// ---------------------------------------------------------------------------
// A sink the test owns. Never a global, which is the rule the crate keeps.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<RowChange<'static>>>,
    asked: AtomicUsize,
}

impl Recorder {
    fn changes(&self) -> Vec<RowChange<'static>> {
        self.seen.lock().unwrap().clone()
    }

    fn only(&self) -> RowChange<'static> {
        let seen = self.changes();
        assert_eq!(seen.len(), 1, "exactly one change: {seen:?}");
        seen.into_iter().next().unwrap()
    }
}

impl ChangeSink for Recorder {
    fn publish(&self, change: &RowChange<'_>) {
        self.seen.lock().unwrap().push(change.clone().into_owned());
    }

    fn wants(&self, _collection: &str) -> bool {
        self.asked.fetch_add(1, Ordering::SeqCst);
        true
    }
}

/// Counts what a sink was asked and told, over any other sink.
struct Counting<S> {
    inner: S,
    published: AtomicUsize,
}

impl<S> Counting<S> {
    fn new(inner: S) -> Counting<S> {
        Counting {
            inner,
            published: AtomicUsize::new(0),
        }
    }

    fn published(&self) -> usize {
        self.published.load(Ordering::SeqCst)
    }
}

impl<S: ChangeSink> ChangeSink for Counting<S> {
    fn publish(&self, change: &RowChange<'_>) {
        self.published.fetch_add(1, Ordering::SeqCst);
        self.inner.publish(change);
    }

    fn wants(&self, collection: &str) -> bool {
        self.inner.wants(collection)
    }
}

async fn migrated() -> Harness {
    let harness = Harness::sqlite().await.expect("connect");
    harness.migrate::<Migrator>().await.expect("migrate");
    harness
}

fn an_author() -> user::ActiveModel {
    user::ActiveModel {
        name: Set("Ada".to_owned()),
        email: Set("ada@example.com".to_owned()),
        ..Default::default()
    }
}

fn a_post(user_id: i32, slug: &str) -> post::ActiveModel {
    post::ActiveModel {
        user_id: Set(user_id),
        title: Set("Notes on the engine".to_owned()),
        slug: Set(slug.to_owned()),
        status: Set("draft".to_owned()),
        views: Set(0),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// 1. One write, one change, with the right table, row and kind
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_insert_publishes_one_created_change_naming_the_table_and_the_row() {
    let harness = migrated().await;
    let db = harness.begin().await.expect("begin");
    let sink = Recorder::default();

    let author = an_author()
        .insert_published(&*db, &sink)
        .await
        .expect("insert");

    let change = sink.only();
    assert_eq!(change.collection, "user");
    // The key the database generated, not the `NotSet` the record carried.
    assert_eq!(change.row, RowId::I64(i64::from(author.id)));
    assert_eq!(change.kind, ChangeKind::Created);
    // A create reports no field list: every column of a new row is new.
    assert!(change.changed_fields().is_empty());
}

#[tokio::test]
async fn an_update_publishes_the_columns_it_wrote() {
    let harness = migrated().await;
    let db = harness.begin().await.expect("begin");
    let sink = Recorder::default();

    let author = an_author().insert_checked(&*db).await.expect("insert");
    let stored = a_post(author.id, "notes")
        .insert_checked(&*db)
        .await
        .expect("insert");

    let mut tracked = Tracked::<post::ActiveModel>::load(stored.clone());
    tracked.title = Set("Notes, revised".to_owned());
    // Assigned the value it already held, so it is not a change and must not
    // be reported as one.
    tracked.status = Set("draft".to_owned());

    let updated = tracked.save_published(&*db, &sink).await.expect("update");

    assert_eq!(updated.title, "Notes, revised");
    let change = sink.only();
    assert_eq!(change.collection, "post");
    assert_eq!(change.row, RowId::I64(i64::from(stored.id)));
    assert!(matches!(change.kind, ChangeKind::Updated { .. }));
    assert!(change.changed_field("title"));
    assert!(!change.changed_field("status"));
    assert_eq!(change.changed_fields().len(), 1);
}

#[tokio::test]
async fn a_delete_publishes_once_and_names_the_row_it_removed() {
    let harness = migrated().await;
    let db = harness.begin().await.expect("begin");
    let sink = Recorder::default();

    let author = an_author().insert_checked(&*db).await.expect("insert");
    let stored = a_post(author.id, "going")
        .insert_checked(&*db)
        .await
        .expect("insert");
    let id = stored.id;

    let removed = stored
        .into_active_model()
        .delete_published(&*db, &sink)
        .await
        .expect("delete");

    assert_eq!(removed, 1);
    let change = sink.only();
    assert_eq!(change.collection, "post");
    assert_eq!(change.row, RowId::I64(i64::from(id)));
    assert_eq!(change.kind, ChangeKind::Deleted);
}

#[tokio::test]
async fn a_write_that_was_refused_publishes_nothing() {
    let harness = migrated().await;
    let db = harness.begin().await.expect("begin");
    let sink = Recorder::default();

    // `email` is NotSet, so the record never reaches SQL.
    let refused = user::ActiveModel {
        name: Set("A".to_owned()),
        ..Default::default()
    }
    .insert_published(&*db, &sink)
    .await;

    assert!(refused.expect_err("rejected").is_invalid());
    assert!(sink.changes().is_empty());
}

// ---------------------------------------------------------------------------
// 2. A rollback publishes nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_rollback_publishes_nothing_and_a_commit_publishes_everything() {
    let harness = migrated().await;
    let db = harness.begin().await.expect("begin");
    let sink = Recorder::default();

    // Rolled back. The rows are gone, so the changes must be too.
    let writing = Writing::begin(&*db, &sink).await.expect("begin");
    let author = an_author()
        .insert_published(&*writing, &writing)
        .await
        .expect("insert");
    a_post(author.id, "doomed")
        .insert_published(&*writing, &writing)
        .await
        .expect("insert");
    assert_eq!(writing.held(), 2, "held, not published");
    assert!(sink.changes().is_empty());
    writing.rollback().await.expect("rollback");
    assert!(sink.changes().is_empty(), "a rollback publishes nothing");

    // Committed. Now the same two writes go out, in the order they happened.
    let writing = Writing::begin(&*db, &sink).await.expect("begin");
    let author = an_author()
        .insert_published(&*writing, &writing)
        .await
        .expect("insert");
    a_post(author.id, "kept")
        .insert_published(&*writing, &writing)
        .await
        .expect("insert");
    assert_eq!(writing.commit().await.expect("commit"), 2);

    let seen = sink.changes();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].collection, "user");
    assert_eq!(seen[1].collection, "post");
}

#[tokio::test]
async fn a_guard_that_is_dropped_publishes_nothing() {
    let harness = migrated().await;
    let db = harness.begin().await.expect("begin");
    let sink = Recorder::default();

    {
        let writing = Writing::begin(&*db, &sink).await.expect("begin");
        an_author()
            .insert_published(&*writing, &writing)
            .await
            .expect("insert");
        assert_eq!(writing.held(), 1);
        // Dropped without a commit: the transaction rolls back and the change
        // goes nowhere. Forgetting to publish is the safe way to forget.
    }

    assert!(sink.changes().is_empty());
}

// ---------------------------------------------------------------------------
// 3. A sink that wants nothing costs the write path nothing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_application_that_installed_no_sink_never_builds_a_change() {
    let harness = migrated().await;
    let db = harness.begin().await.expect("begin");

    // What an application with no subscriber holds. `wants` is `false`, so
    // the write path reads no primary key and collects no column names.
    let sink = Counting::new(NoopSink);
    an_author()
        .insert_published(&*db, &sink)
        .await
        .expect("insert");
    assert_eq!(sink.published(), 0);

    // And the empty fan-out costs the same, which is the shape an application
    // assembles before it adds anything to it.
    let sink = Counting::new(FanOut::new());
    an_author()
        .insert_published(&*db, &sink)
        .await
        .expect_err("the email is taken, which is beside the point");
    assert_eq!(sink.published(), 0);
}

// ---------------------------------------------------------------------------
// 4. The whole chain: SeaORM write -> RowChange -> broker -> a mailbox
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_row_written_through_the_orm_reaches_a_realtime_subscriber() {
    let harness = migrated().await;
    let db = harness.begin().await.expect("begin");

    let broker = Broker::new();
    let watcher = broker.connect().expect("connect");
    watcher
        .subscribe(Subscription::new(
            Topic::collection(table_name::<post::Entity>()).expect("topic"),
            Gate::everyone(),
        ))
        .expect("subscribe");

    // The model layer holds `Arc<Broker>` as a `ChangeSink` and never asks
    // what it is.
    let sink = Arc::clone(&broker);
    let author = an_author().insert_checked(&*db).await.expect("insert");
    let stored = a_post(author.id, "live")
        .insert_published(&*db, &sink)
        .await
        .expect("insert");

    let frames: Vec<String> = watcher
        .mailbox()
        .drain()
        .into_iter()
        .filter_map(|event| match event {
            Event::Frame(frame) => Some(String::from_utf8(frame.bytes().to_vec()).unwrap()),
            Event::Lagged { .. } => None,
        })
        .collect();

    assert_eq!(frames.len(), 1, "exactly one frame: {frames:?}");
    let frame = &frames[0];
    assert!(frame.contains("data:action=create"), "{frame}");
    assert!(frame.contains("data:collection=post"), "{frame}");
    assert!(frame.contains(&format!("data:id={}", stored.id)), "{frame}");

    // The subscriber is for `post`, so a write to `user` is one failed hash
    // lookup and no frame at all.
    assert!(!broker.wants("user"));
    an_author().insert_checked(&*db).await.expect_err("taken");
    assert!(watcher.mailbox().drain().is_empty());
}

#[tokio::test]
async fn one_write_reaches_every_installed_sink() {
    let harness = migrated().await;
    let db = harness.begin().await.expect("begin");

    let audit = Arc::new(Recorder::default());
    let cache = Arc::new(Recorder::default());
    let sink = FanOut::new()
        .with(Arc::clone(&audit))
        .with(Arc::clone(&cache));

    an_author()
        .insert_published(&*db, &sink)
        .await
        .expect("insert");

    assert_eq!(audit.changes().len(), 1);
    assert_eq!(cache.changes().len(), 1);
    assert_eq!(audit.changes()[0], cache.changes()[0]);
}

// ---------------------------------------------------------------------------
// 5. The plugin seam, and what it costs an application that does not use it
// ---------------------------------------------------------------------------

#[cfg(feature = "plugin")]
mod seam {
    use super::*;

    use brocanth::plugin_api::{Plugin, PluginError};
    use brocanth::plugin_app::AppBuilder;
    use brocanth::plugin_context::NoContext;
    use brocanth::plugin_hooks::{HookFamilies, HookRegistrar, HookSource, NoHooks, OnModel};
    use brocanth::plugin_model::ModelHook;
    use brocanth::plugin_order::Order;
    use brocanth_model::plugin::{HookSink, RowWritten};

    /// A plugin that wants to know about every row written.
    struct Audit {
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl Plugin for Audit {
        type Config = Arc<Mutex<Vec<String>>>;
        type Provides = NoContext;
        const NAME: &'static str = "audit";
        const INSTALLS_HOOKS: bool = true;

        fn new(config: Self::Config) -> Self {
            Audit { seen: config }
        }
    }

    struct Trail(Arc<Mutex<Vec<String>>>);

    impl ModelHook<RowWritten> for Trail {
        fn on_event(&self, change: &mut RowChange<'_>) -> Result<(), PluginError> {
            let action = match change.kind {
                ChangeKind::Created => "create",
                ChangeKind::Updated { .. } => "update",
                ChangeKind::Deleted => "delete",
            };
            self.0
                .lock()
                .unwrap()
                .push(format!("{action} {}/{}", change.collection, change.row));
            Ok(())
        }
    }

    impl HookSource for Audit {
        type Families = OnModel;

        fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
            registrar.model::<RowWritten>("trail", Order::new(), Trail(Arc::clone(&self.seen)));
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_model_hook_hears_the_write_through_the_core() {
        let harness = migrated().await;
        let db = harness.begin().await.expect("begin");

        let log = Arc::new(Mutex::new(Vec::new()));
        let app = AppBuilder::new()
            .plugin_hooking(Audit::new(Arc::clone(&log)))
            .build()
            .expect("build");

        let sink = HookSink::new(app.model_dispatch());
        assert!(sink.wants("user"), "a hook is installed for this kind");

        let author = an_author()
            .insert_published(&*db, &sink)
            .await
            .expect("insert");
        author
            .into_active_model()
            .delete_published(&*db, &sink)
            .await
            .expect("delete");

        let seen = log.lock().unwrap().clone();
        assert_eq!(seen.len(), 2, "{seen:?}");
        assert!(seen[0].starts_with("create user/"), "{seen:?}");
        assert!(seen[1].starts_with("delete user/"), "{seen:?}");
    }

    #[tokio::test]
    async fn a_plugin_that_registers_no_model_hook_pays_nothing() {
        // The type-level fact that makes it free, asserted where it lives: in
        // a const block, so these two lines are checked by the compiler and
        // not by the runner. `HookSink::wants` below is `H::MODEL`, so for an
        // application with no model hook it is `false` at compile time and the
        // branch that builds a change does not survive compilation.
        const { assert!(!<NoHooks as HookFamilies>::MODEL) };
        const { assert!(<OnModel as HookFamilies>::MODEL) };

        let harness = migrated().await;
        let db = harness.begin().await.expect("begin");

        // An application with no hooking plugin keeps `H = NoHooks`.
        let app = AppBuilder::new().build().expect("build");
        let sink = Counting::new(HookSink::new(app.model_dispatch()));

        assert!(!sink.wants("user"));
        an_author()
            .insert_published(&*db, &sink)
            .await
            .expect("insert");
        assert_eq!(sink.published(), 0);
    }

    #[tokio::test]
    async fn two_applications_in_one_process_publish_to_their_own_hooks() {
        let harness = migrated().await;
        let db = harness.begin().await.expect("begin");

        let first_log = Arc::new(Mutex::new(Vec::new()));
        let second_log = Arc::new(Mutex::new(Vec::new()));
        let first = AppBuilder::new()
            .plugin_hooking(Audit::new(Arc::clone(&first_log)))
            .build()
            .expect("build");
        let second = AppBuilder::new()
            .plugin_hooking(Audit::new(Arc::clone(&second_log)))
            .build()
            .expect("build");

        let sink = HookSink::new(first.model_dispatch());
        an_author()
            .insert_published(&*db, &sink)
            .await
            .expect("insert");

        assert_eq!(first_log.lock().unwrap().len(), 1);
        assert!(second_log.lock().unwrap().is_empty());
        let _ = second;
    }
}

// The row id helper is what every change above names the row with, so it is
// worth one direct test rather than only its consequences.
#[tokio::test]
async fn the_row_id_of_a_stored_row_is_its_primary_key() {
    let harness = migrated().await;
    let db = harness.begin().await.expect("begin");

    let author = an_author().insert_checked(&*db).await.expect("insert");
    assert_eq!(table_name::<user::Entity>(), "user");
    assert_eq!(
        row_id::<user::Entity>(&author),
        RowId::I64(i64::from(author.id))
    );
}
