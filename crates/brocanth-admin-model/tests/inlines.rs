//! Inlines on a real database: a user's posts on the user's form, with the
//! post's row policy asked per child row.
//!
//! SQLite always. PostgreSQL too when `BROCANTH_TEST_DATABASE_URL` (or
//! `BROCANTH_MODEL_TEST_POSTGRES`) names a server: the test makes a schema
//! of its own there and drops it at the end.
//!
//! The policy: anybody may list posts, a post may be changed or deleted
//! while it is a draft, and a post may be added.

use std::sync::Arc;

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::NoHooks;
use brocanth_admin::data::AdminData;
use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::row_policy::Dialect;
use brocanth_admin::{AdminConfig, AdminPlugin, AdminResource, AdminSetup, Discovery, Inline};
use brocanth_admin_model::admin_source;
use brocanth_model::sea_orm::{ConnectionTrait, DatabaseConnection};
use brocanth_model::testing::Harness;
use brocanth_test::TestClient;

mod blog {
    pub mod user {
        use brocanth_model::prelude::*;
        use serde::{Deserialize, Serialize};

        #[sea_orm::model]
        #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
        #[sea_orm(table_name = "person")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub name: String,
        }

        impl ActiveModelBehavior for ActiveModel {}
    }

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
    /// The two entities, one line each.
    pub struct Models;
    "blog::person" => blog::user::Entity,
    "blog::post" => blog::post::Entity,
}

brocanth_policy::policy! {
    #[model(blog::user::Model)]
    entity Person in "person" {
        #[key] id: i32,
        name: String,
    }

    #[model(blog::post::Model)]
    entity Post in "post" {
        #[key] id: i32,
        author_id: i32,
        title: String,
        status: String,
    }

    policy PersonRules(_who: &()) for Person {
        list: always,
        update: always,
        writable { except [id] }
    }

    policy PostRules(_who: &()) for Post {
        list: always,
        create: always,
        update: status == "draft",
        delete: status == "draft",
        writable { except [id] }
    }
}

/// One resource pair per dialect: the dialect is a constant of the type.
macro_rules! resources {
    ($person:ident, $post:ident, $dialect:expr) => {
        struct $person;
        impl AdminResource for $person {
            type Entity = Person;
            type Subject = ();
            type Policy = PersonRules;
            const MODEL: &'static str = "person";
            const DIALECT: Dialect = $dialect;
            fn policy(&self) -> &PersonRules {
                &PersonRules
            }
            fn subject<'a>(&self, _: &brocanth_admin::Principal<'a>) -> Option<&'a ()> {
                Some(&())
            }
        }
        struct $post;
        impl AdminResource for $post {
            type Entity = Post;
            type Subject = ();
            type Policy = PostRules;
            const MODEL: &'static str = "post";
            const DIALECT: Dialect = $dialect;
            fn policy(&self) -> &PostRules {
                &PostRules
            }
            fn subject<'a>(&self, _: &brocanth_admin::Principal<'a>) -> Option<&'a ()> {
                Some(&())
            }
        }
    };
}

resources!(SqlitePeople, SqlitePosts, Dialect::Sqlite);
resources!(PgPeople, PgPosts, Dialect::Postgres);

fn client(config: AdminConfig, data: Arc<dyn AdminData>) -> TestClient<Base, NoHooks> {
    let config = config
        .discovering(Discovery::new().under("blog"))
        .with_policy(AllowAllUnsafeForDevelopment)
        .model("person", |m| {
            m.inline(
                Inline::new("post")
                    .fk("author_id")
                    .fields(["title", "status"]),
            )
        });
    let mut app = AppBuilder::new()
        .plugin(AdminPlugin::new(AdminSetup::new(data, config)))
        .build()
        .expect("an application");
    app.mount_routes::<AdminPlugin>("/admin")
        .expect("the admin mounts");
    TestClient::new(app)
}

async fn seed(conn: &DatabaseConnection, schema: &[&str]) {
    for statement in schema {
        conn.execute_unprepared(statement).await.expect("schema");
    }
    for statement in [
        "INSERT INTO person (id, name) VALUES (1, 'Ada'), (2, 'Grace')",
        "INSERT INTO post (id, author_id, title, status) VALUES \
         (1, 1, 'Draft one', 'draft'), \
         (2, 1, 'Out already', 'published'), \
         (3, 2, 'Not Ada''s', 'draft')",
    ] {
        conn.execute_unprepared(statement).await.expect("rows");
    }
}

async fn posts(conn: &DatabaseConnection) -> Vec<(i32, i32, String, String)> {
    use brocanth_model::sea_orm::{EntityTrait, QueryOrder};
    blog::post::Entity::find()
        .order_by_asc(blog::post::Column::Id)
        .all(conn)
        .await
        .expect("posts")
        .into_iter()
        .map(|p| (p.id, p.author_id, p.title, p.status))
        .collect()
}

/// The whole story, on one database.
async fn inline_rows_obey_the_child_policy(
    conn: &DatabaseConnection,
    client: TestClient<Base, NoHooks>,
) {
    // The draft is editable and deletable; the published post is shown,
    // read-only, and posts nothing. Grace's post is not on Ada's form.
    let page = client.get_async("/admin/person/1/edit").await;
    page.assert_ok();
    let html = page.html();
    html.assert_field("post-0-title")
        .assert_field("post-0-delete")
        .assert_text("Out already")
        .assert_no_text("Not Ada's");
    assert!(html.field("post-1-title").is_none());
    assert!(html.field("post-1-pk").is_none());
    assert!(html.field("post-1-delete").is_none());

    // Changing the published post anyway: 403, and nothing is written.
    let crafted = client
        .post_async(
            "/admin/person/1",
            &[
                ("name", "Ada"),
                ("post-0-pk", "1"),
                ("post-0-title", "Draft, changed"),
                ("post-0-status", "draft"),
                ("post-1-pk", "2"),
                ("post-1-title", "Rewritten"),
                ("post-1-status", "published"),
            ],
        )
        .await;
    crafted.assert_status(403);
    assert_eq!(posts(conn).await[0].2, "Draft one");
    assert_eq!(posts(conn).await[1].2, "Out already");

    // One POST: the parent, a change to the draft, and a new post.
    client
        .post_async(
            "/admin/person/1",
            &[
                ("name", "Ada Lovelace"),
                ("post-0-pk", "1"),
                ("post-0-title", "Draft, changed"),
                ("post-0-status", "draft"),
                ("post-2-title", "Brand new"),
                ("post-2-status", "draft"),
            ],
        )
        .await
        .assert_redirects("/admin/person/1");
    let now = posts(conn).await;
    assert_eq!(now[0], (1, 1, "Draft, changed".into(), "draft".into()));
    assert_eq!(now[1].2, "Out already");
    assert_eq!(now[2].2, "Not Ada's");
    assert_eq!(now[3].1, 1, "the new post points at Ada");
    assert_eq!(now[3].2, "Brand new");

    // Deleting the draft.
    client
        .post_async(
            "/admin/person/1",
            &[
                ("name", "Ada Lovelace"),
                ("post-0-pk", "1"),
                ("post-0-delete", "on"),
            ],
        )
        .await
        .assert_redirects("/admin/person/1");
    assert!(
        posts(conn).await.iter().all(|p| p.0 != 1),
        "the draft is gone"
    );
}

const SQLITE: [&str; 2] = [
    "CREATE TABLE person (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL)",
    "CREATE TABLE post (id INTEGER PRIMARY KEY AUTOINCREMENT, author_id INTEGER NOT NULL, \
     title TEXT NOT NULL, status TEXT NOT NULL)",
];

#[tokio::test]
async fn sqlite_inline_rows_obey_the_child_policy() {
    let harness = Harness::sqlite().await.expect("connect");
    seed(harness.conn(), &SQLITE).await;
    let data: Arc<dyn AdminData> = Arc::new(Models::new(harness.conn().clone()));
    let config = AdminConfig::new()
        .resource(SqlitePeople)
        .resource(SqlitePosts);
    inline_rows_obey_the_child_policy(harness.conn(), client(config, data)).await;
}

/// A PostgreSQL schema of the test's own, seeded, with the sequences moved
/// past the seeded ids. `None` when no server is named.
///
/// Returns the connection that owns the schema, to drop it at the end, and
/// the connection whose search path is the schema.
async fn postgres_schema(name: &str, extra: &[&str]) -> Option<(Harness, Harness, String)> {
    let Some(url) = Harness::postgres_url() else {
        brocanth_model::testing::skipped("no BROCANTH_TEST_DATABASE_URL");
        return None;
    };
    // A schema of this test's own, named in the connection's search path,
    // so nothing here touches a shared table.
    let schema = format!("{name}_{}", std::process::id());
    let admin = Harness::postgres(&url).await.expect("connect");
    admin
        .conn()
        .execute_unprepared(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}"
        ))
        .await
        .expect("schema");
    let separator = if url.contains('?') { '&' } else { '?' };
    let scoped = format!("{url}{separator}options=-csearch_path%3D{schema}");
    let harness = Harness::postgres(&scoped)
        .await
        .expect("connect in the schema");
    let mut tables = vec![
        "CREATE TABLE person (id SERIAL PRIMARY KEY, name TEXT NOT NULL)",
        "CREATE TABLE post (id SERIAL PRIMARY KEY, author_id INTEGER NOT NULL, \
         title TEXT NOT NULL, status TEXT NOT NULL)",
    ];
    tables.extend_from_slice(extra);
    seed(harness.conn(), &tables).await;
    // The rows were inserted with their ids; the sequences start after them.
    for table in ["person", "post"] {
        harness
            .conn()
            .execute_unprepared(&format!(
                "SELECT setval(pg_get_serial_sequence('{table}', 'id'), (SELECT max(id) FROM {table}))"
            ))
            .await
            .expect("sequence");
    }
    Some((admin, harness, schema))
}

async fn drop_schema(admin: &Harness, schema: &str) {
    admin
        .conn()
        .execute_unprepared(&format!("DROP SCHEMA {schema} CASCADE"))
        .await
        .expect("drop the schema");
}

#[tokio::test]
async fn postgres_inline_rows_obey_the_child_policy() {
    let Some((admin, harness, schema)) = postgres_schema("admin_inline", &[]).await else {
        return;
    };
    let data: Arc<dyn AdminData> = Arc::new(Models::new(harness.conn().clone()));
    let config = AdminConfig::new().resource(PgPeople).resource(PgPosts);
    inline_rows_obey_the_child_policy(harness.conn(), client(config, data)).await;
    drop_schema(&admin, &schema).await;
}

/// The name of person 1.
async fn person_name(conn: &DatabaseConnection) -> String {
    use brocanth_model::sea_orm::EntityTrait;
    blog::user::Entity::find_by_id(1)
        .one(conn)
        .await
        .expect("a read")
        .expect("person 1")
        .name
}

#[tokio::test]
async fn postgres_a_refused_child_row_undoes_the_parent_and_the_rows_before_it() {
    // A unique title: the database refuses the third write of the save.
    let Some((admin, harness, schema)) = postgres_schema(
        "admin_inline_atomic",
        &["CREATE UNIQUE INDEX post_title ON post (title)"],
    )
    .await
    else {
        return;
    };
    let conn = harness.conn();
    let data: Arc<dyn AdminData> = Arc::new(Models::new(conn.clone()));
    let config = AdminConfig::new().resource(PgPeople).resource(PgPosts);
    let client = client(config, data);
    let before = posts(conn).await;

    // The parent changes, the draft changes, then a new post repeats the
    // title of post 2: the index refuses it.
    let response = client
        .post_async(
            "/admin/person/1",
            &[
                ("name", "Ada Lovelace"),
                ("post-0-pk", "1"),
                ("post-0-title", "Draft, changed"),
                ("post-0-status", "draft"),
                ("post-2-title", "Out already"),
                ("post-2-status", "draft"),
            ],
        )
        .await;
    assert!(
        response.status() >= 500,
        "the database refused the row: {}",
        response.status()
    );
    assert_eq!(person_name(conn).await, "Ada", "the parent is not kept");
    assert_eq!(posts(conn).await, before, "no child write is kept");

    // The same save with a new title is kept whole.
    client
        .post_async(
            "/admin/person/1",
            &[
                ("name", "Ada Lovelace"),
                ("post-0-pk", "1"),
                ("post-0-title", "Draft, changed"),
                ("post-0-status", "draft"),
                ("post-2-title", "Brand new"),
                ("post-2-status", "draft"),
            ],
        )
        .await
        .assert_redirects("/admin/person/1");
    assert_eq!(person_name(conn).await, "Ada Lovelace");
    let now = posts(conn).await;
    assert_eq!(now[0].2, "Draft, changed");
    assert_eq!(now.len(), 4);
    drop_schema(&admin, &schema).await;
}
