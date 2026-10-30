//! The admin against row-level policies, on a real database, through the
//! router.
//!
//! PLAN.md §12 item 9. The claim is that the policy's condition reaches the
//! SQL, so the tests run against SQLite through SeaORM rather than against
//! the in-memory fixture, which refuses a condition outright. They drive the
//! admin with `brocanth-test`'s client, so the router, the plugin and the
//! principal hook are on the path, as they are in production.
//!
//! One policy, written the way an application would:
//!
//! * a staff member lists and views only their own posts; a superuser sees
//!   every post;
//! * a post may be changed by its author while it is a draft;
//! * a staff member may write the title and nothing else.

use std::sync::Arc;

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::{NoHooks, Request, RequestState};
use brocanth_admin::data::{AdminData, DataError, ListRequest, Scope, Search};
use brocanth_admin::discovery::RegistryEntry;
use brocanth_admin::model::{ModelDescriptor, PrimaryKey, Row, Value};
use brocanth_admin::policy::{policy_fn, Principal};
use brocanth_admin::row_policy::{Dialect, Policy};
use brocanth_admin::{AdminConfig, AdminPlugin, AdminResource, AdminSetup, Discovery};
use brocanth_admin_model::{admin_source, async_trait};
use brocanth_model::sea_orm::{ConnectionTrait, DatabaseConnection};
use brocanth_model::testing::Harness;
use brocanth_test::TestClient;

mod blog {
    pub mod post {
        use brocanth_model::prelude::*;
        use serde::{Deserialize, Serialize};

        #[sea_orm::model]
        #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
        #[sea_orm(table_name = "post")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub author_id: i32,
            pub title: String,
            pub status: String,
        }

        impl ActiveModelBehavior for ActiveModel {}
    }
}

admin_source! {
    /// The one entity, one line.
    pub struct Models;
    "blog::post" => blog::post::Entity,
}

const SCHEMA: &str = "CREATE TABLE post (id INTEGER PRIMARY KEY AUTOINCREMENT, \
     author_id INTEGER NOT NULL, title TEXT NOT NULL, status TEXT NOT NULL)";

// ---------------------------------------------------------------------------
// The policy, as an application writes it
// ---------------------------------------------------------------------------

/// What the application's authentication hook put in the request state.
#[derive(Clone, Copy, Debug)]
struct Staff {
    id: i64,
    superuser: bool,
}

// The entity is checked against the SeaORM model: each column is a field of
// `blog::post::Model` with the same type. So the names the admin matches at
// runtime (`Column::name()` against its own column names) come from the
// same struct, and a typo or a changed type is a compile error.
brocanth_policy::policy! {
    #[model(blog::post::Model)]
    entity Post in "post" {
        #[key] id: i32,
        author_id: i32,
        title: String,
        status: String,
    }

    policy PostRules(who: &Staff) for Post {
        list {
            who.superuser => always,
            author_id == who.id,
        }
        update: author_id == who.id && status == "draft",
        delete: author_id == who.id,
        writable {
            who.superuser => except [id],
            only [title],
        }
    }
}

struct Posts;

impl AdminResource for Posts {
    type Entity = Post;
    type Subject = Staff;
    type Policy = PostRules;

    const MODEL: &'static str = "post";
    const DIALECT: Dialect = Dialect::Sqlite;

    fn policy(&self) -> &PostRules {
        &PostRules
    }
}

// ---------------------------------------------------------------------------
// The application around it
// ---------------------------------------------------------------------------

const ADA: Staff = Staff {
    id: 1,
    superuser: false,
};
const ROOT: Staff = Staff {
    id: 99,
    superuser: true,
};

async fn database() -> Harness {
    let harness = Harness::sqlite().await.expect("connect");
    harness
        .conn()
        .execute_unprepared(SCHEMA)
        .await
        .expect("schema");
    harness
}

async fn post(conn: &DatabaseConnection, author: i64, title: &str, status: &str) -> i64 {
    let sql = format!(
        "INSERT INTO post (author_id, title, status) VALUES ({author}, '{title}', '{status}')"
    );
    let done = conn.execute_unprepared(&sql).await.expect("insert");
    i64::try_from(done.last_insert_id()).expect("an id")
}

async fn column(conn: &DatabaseConnection, id: i64, name: &str) -> String {
    let models = Models::new(conn.clone());
    let row = models
        .fetch(
            &descriptor(&models),
            &PrimaryKey::single("id", id),
            Scope::Unscoped,
        )
        .await
        .expect("fetch")
        .expect("the row");
    row.get(name).map(Value::as_input).unwrap_or_default()
}

fn descriptor(data: &dyn AdminData) -> ModelDescriptor {
    brocanth_admin::discover(data.registry(), &Discovery::new())
        .into_iter()
        .find(|d| d.key == "post")
        .expect("the post model")
}

/// The admin, mounted at `/admin` over `data`, with a client signed in as
/// `who`.
fn client(data: Arc<dyn AdminData>, who: Staff) -> TestClient<Base, NoHooks> {
    let config = AdminConfig::new()
        .discovering(Discovery::new().under("blog"))
        .with_policy(policy_fn(|who: &Principal<'_>, _| {
            who.downcast::<Staff>().is_some()
        }))
        .model("post", |m| m.per_page(3))
        .resource(Posts);
    let setup = AdminSetup::new(data, config).principal(|_: &Request, state: &RequestState| {
        state
            .get::<Staff>()
            .map_or_else(Principal::anonymous, |s| Principal::new(s))
    });
    let mut app = AppBuilder::new()
        .plugin(AdminPlugin::new(setup))
        .build()
        .expect("an application");
    app.mount_routes::<AdminPlugin>("/admin")
        .expect("the admin mounts");
    TestClient::new(app).seeding(move |state: &mut RequestState| {
        state.insert(who);
    })
}

/// The ids a list page links to, in order.
fn ids_on(html: &brocanth_test::html::Html) -> Vec<i64> {
    html.links()
        .iter()
        .filter_map(|a| a.href()?.strip_prefix("/admin/post/")?.parse().ok())
        .fold(Vec::new(), |mut ids, id| {
            if !ids.contains(&id) {
                ids.push(id);
            }
            ids
        })
}

// ---------------------------------------------------------------------------
// The list: the condition is in the query, so the totals do not lie
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_list_is_filtered_in_the_query_and_its_totals_match_its_pages() {
    let db = database().await;
    // Interleaved, so a paginator that filtered after reading would get
    // pages of mixed size and a total of twelve.
    let mut own = Vec::new();
    for n in 0..12 {
        let author = if n % 2 == 0 || n == 11 { 1 } else { 2 };
        let id = post(db.conn(), author, &format!("post {n}"), "draft").await;
        if author == 1 {
            own.push(id);
        }
    }
    assert_eq!(own.len(), 7);

    let ada = client(Arc::new(Models::new(db.conn().clone())), ADA);
    let mut seen = Vec::new();
    for (page, expected) in [(1, "1–3 of 7"), (2, "4–6 of 7"), (3, "7–7 of 7")] {
        let response = ada.get_async(&format!("/admin/post?page={page}")).await;
        response.assert_ok();
        let html = response.html();
        html.assert_text(expected)
            .assert_text(&format!("page {page} of 3"));
        let ids = ids_on(&html);
        assert_eq!(ids.len(), if page == 3 { 1 } else { 3 }, "page {page}");
        for id in ids {
            assert!(!seen.contains(&id), "page {page} repeats post {id}");
            seen.push(id);
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, own, "the three pages are exactly Ada's seven posts");

    // The same screen, for somebody whose rule is `Always`.
    let root = client(Arc::new(Models::new(db.conn().clone())), ROOT);
    root.get_async("/admin/post")
        .await
        .assert_ok()
        .html()
        .assert_text("1–3 of 12");
}

// ---------------------------------------------------------------------------
// One row: a guarded select, and 404 for a row the rule refuses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_forbidden_row_is_not_found_on_every_screen_that_reads_one() {
    let db = database().await;
    let mine = post(db.conn(), 1, "mine", "draft").await;
    let theirs = post(db.conn(), 2, "not yours", "draft").await;
    let ada = client(Arc::new(Models::new(db.conn().clone())), ADA);

    ada.get_async(&format!("/admin/post/{mine}"))
        .await
        .assert_ok()
        .html()
        .assert_text("mine");

    // A row that is there and refused, and a row that is not there, are one
    // answer.
    for path in [
        format!("/admin/post/{theirs}"),
        format!("/admin/post/{theirs}/edit"),
        format!("/admin/post/{theirs}/delete"),
        "/admin/post/4040".to_string(),
    ] {
        let response = ada.get_async(&path).await;
        response.assert_status(404);
        response.html().assert_no_text("not yours");
    }

    // The writes get the same answer, and write nothing.
    ada.post_async(&format!("/admin/post/{theirs}"), &[("title", "mine now")])
        .await
        .assert_status(404);
    ada.post_async(&format!("/admin/post/{theirs}/delete"), &[])
        .await
        .assert_status(404);
    assert_eq!(column(db.conn(), theirs, "title").await, "not yours");

    // And the row that is hers can be deleted.
    ada.post_async(&format!("/admin/post/{mine}/delete"), &[])
        .await
        .assert_redirects("/admin/post");
}

// ---------------------------------------------------------------------------
// The write: guarded by the condition it was checked against
// ---------------------------------------------------------------------------

/// The SeaORM source, with another writer that publishes the post between
/// the admin's check and its write. That is the race PLAN.md §9 obligation 2
/// is about, made to happen every time.
struct Racing {
    inner: Models,
}

#[async_trait]
impl AdminData for Racing {
    fn registry(&self) -> Vec<RegistryEntry> {
        self.inner.registry()
    }

    async fn count(
        &self,
        model: &ModelDescriptor,
        search: Search<'_>,
        scope: Scope<'_>,
    ) -> Result<usize, DataError> {
        self.inner.count(model, search, scope).await
    }

    async fn list(&self, request: ListRequest<'_>) -> Result<Vec<Row>, DataError> {
        self.inner.list(request).await
    }

    async fn fetch(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: Scope<'_>,
    ) -> Result<Option<Row>, DataError> {
        self.inner.fetch(model, key, scope).await
    }

    async fn insert(
        &self,
        model: &ModelDescriptor,
        values: &[(String, Value)],
    ) -> Result<PrimaryKey, DataError> {
        self.inner.insert(model, values).await
    }

    async fn update(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        values: &[(String, Value)],
        scope: Scope<'_>,
    ) -> Result<u64, DataError> {
        let id = key.get("id").expect("an id");
        self.inner
            .connection()
            .execute_unprepared(&format!(
                "UPDATE post SET status = 'published' WHERE id = {id}"
            ))
            .await
            .map_err(|e| DataError::backend(e.to_string()))?;
        self.inner.update(model, key, values, scope).await
    }

    async fn delete(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: Scope<'_>,
    ) -> Result<u64, DataError> {
        self.inner.delete(model, key, scope).await
    }
}

#[tokio::test]
async fn a_guarded_update_that_changes_no_row_is_a_conflict_and_writes_nothing() {
    let db = database().await;
    let id = post(db.conn(), 1, "draft title", "draft").await;

    // Without the race, the same request succeeds.
    let calm = client(Arc::new(Models::new(db.conn().clone())), ADA);
    calm.post_async(&format!("/admin/post/{id}"), &[("title", "first edit")])
        .await
        .assert_redirects(&format!("/admin/post/{id}"));
    assert_eq!(column(db.conn(), id, "title").await, "first edit");

    // The check passes on a draft; the write finds a published post, which
    // the update rule refuses. Zero rows, so 409 and not a silent success.
    let racing = Racing {
        inner: Models::new(db.conn().clone()),
    };
    let ada = client(Arc::new(racing), ADA);
    ada.post_async(&format!("/admin/post/{id}"), &[("title", "second edit")])
        .await
        .assert_status(409);
    assert_eq!(
        column(db.conn(), id, "title").await,
        "first edit",
        "the guarded write changed nothing"
    );
    assert_eq!(column(db.conn(), id, "status").await, "published");
}

#[tokio::test]
async fn the_source_reports_zero_rows_for_a_write_the_scope_refuses() {
    // The same guarantee one layer down, without the admin: a scope that
    // does not match the row is zero rows, for an update and for a delete,
    // never an error that reads as success and never a write.
    let db = database().await;
    let id = post(db.conn(), 2, "theirs", "draft").await;
    let models = Models::new(db.conn().clone());
    let model = descriptor(&models);
    let key = PrimaryKey::single("id", id);
    let condition = PostRules
        .update(&ADA)
        .condition::<Post>(Dialect::Sqlite)
        .expect("compiles");

    let changed = models
        .update(
            &model,
            &key,
            &[("title".into(), Value::Text("taken".into()))],
            Scope::Where(&condition),
        )
        .await
        .expect("update");
    assert_eq!(changed, 0);
    let removed = models
        .delete(&model, &key, Scope::Where(&condition))
        .await
        .expect("delete");
    assert_eq!(removed, 0);
    assert_eq!(column(db.conn(), id, "title").await, "theirs");

    // And a condition for the wrong dialect is refused, not run.
    let postgres = PostRules
        .update(&ADA)
        .condition::<Post>(Dialect::Postgres)
        .expect("compiles");
    let refused = models.fetch(&model, &key, Scope::Where(&postgres)).await;
    assert!(matches!(refused, Err(DataError::Backend(_))), "{refused:?}");
}

// ---------------------------------------------------------------------------
// Field-level: `Policy::writable` on the form and on the POST
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_form_offers_only_writable_fields_and_a_post_with_another_is_refused() {
    let db = database().await;
    let id = post(db.conn(), 1, "a draft", "draft").await;
    let ada = client(Arc::new(Models::new(db.conn().clone())), ADA);

    // The edit form has an input for the title and none for the columns
    // the mask refuses. They are shown, as text.
    let form = ada.get_async(&format!("/admin/post/{id}/edit")).await;
    form.assert_ok();
    let html = form.html();
    html.assert_field("title");
    assert!(
        html.field("author_id").is_none(),
        "author_id is not an input"
    );
    assert!(html.field("status").is_none(), "status is not an input");
    html.assert_text("draft");

    // A POST that sends a locked column is refused before any row is read,
    // and nothing is written — not even the title it was allowed to set.
    let refused = ada
        .post_async(
            &format!("/admin/post/{id}"),
            &[("title", "retitled"), ("author_id", "2")],
        )
        .await;
    refused.assert_status(403);
    refused.html().assert_text("author_id");
    assert_eq!(column(db.conn(), id, "author_id").await, "1");
    assert_eq!(column(db.conn(), id, "title").await, "a draft");

    // The same 403 for a row that does not exist: the mask is checked first,
    // so the answer says nothing about the row.
    ada.post_async("/admin/post/4040", &[("status", "published")])
        .await
        .assert_status(403);

    // The title alone goes through.
    ada.post_async(&format!("/admin/post/{id}"), &[("title", "retitled")])
        .await
        .assert_redirects(&format!("/admin/post/{id}"));
    assert_eq!(column(db.conn(), id, "title").await, "retitled");

    // The mask is per subject. A superuser's allows the status, so on their
    // own draft the same column is an input.
    let theirs = post(db.conn(), ROOT.id, "the superuser draft", "draft").await;
    let root = client(Arc::new(Models::new(db.conn().clone())), ROOT);
    let html = root
        .get_async(&format!("/admin/post/{theirs}/edit"))
        .await
        .html();
    html.assert_field("title").assert_field("status");
}

#[tokio::test]
async fn a_rule_that_is_never_is_forbidden_without_reading_a_row() {
    // `Policy::create` is not written, so it is `Never`: 403 on the form and
    // on the POST, and no "add" link on the list.
    let db = database().await;
    let ada = client(Arc::new(Models::new(db.conn().clone())), ADA);
    ada.get_async("/admin/post/new").await.assert_status(403);
    ada.post_async("/admin/post", &[("title", "new")])
        .await
        .assert_status(403);
    ada.get_async("/admin/post")
        .await
        .assert_ok()
        .html()
        .assert_no_link_to("/admin/post/new");
}
