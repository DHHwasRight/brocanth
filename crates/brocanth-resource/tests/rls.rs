//! A resource over a table under forced row-level security: each operation
//! runs in one transaction as the request's `DbIdentity`, so the rows are
//! there with the identity and gone without it.
//!
//! docs/policy-rls-bridge.md, decision 13. The subject is an administrator,
//! so the application's own rule admits every row, and what is left out is
//! what the database's policy left out.

mod common;

use brocanth_db::DbIdentity;
use brocanth_resource::page::PageRequest;
use brocanth_resource::status::{outcome, Action};
use brocanth_resource::{flow, Failure};
use common::{row_ids, seed_eight, Posts, Recorder, Visitor};

fn rls_posts() -> Option<(Posts, String)> {
    let (db, roles) = common::Db::with_rls()?;
    Some((
        Posts {
            db,
            sink: Recorder::listening(),
        },
        roles.member,
    ))
}

/// The member role, as user `id`.
fn as_user(member: &str, id: i64) -> Option<DbIdentity> {
    Some(DbIdentity::new().role(member).set("user_id", id))
}

#[test]
fn a_list_shows_the_rows_of_the_identity_and_none_without_one() {
    let Some((posts, member)) = rls_posts() else {
        return;
    };
    seed_eight(&posts);
    let admin = Visitor::admin();

    let mine = flow::list_as(
        &posts,
        &as_user(&member, 7),
        &admin,
        &PageRequest::new(),
        None,
    )
    .expect("a listing");
    assert_eq!(row_ids(&mine.items), vec![1, 2]);
    assert_eq!(mine.total.count(), Some(2), "the total counts the same set");

    let theirs = flow::list_as(
        &posts,
        &as_user(&member, 9),
        &admin,
        &PageRequest::new(),
        None,
    )
    .expect("a listing");
    assert_eq!(row_ids(&theirs.items), vec![6, 7, 8]);

    // No identity: the login role, which no policy admits.
    let none = flow::list(&posts, &admin, &PageRequest::new(), None).expect("a listing");
    assert!(none.items.is_empty(), "{:?}", none.items);
    assert_eq!(none.total.count(), Some(0));
}

#[test]
fn a_view_finds_the_row_only_as_its_author() {
    let Some((posts, member)) = rls_posts() else {
        return;
    };
    seed_eight(&posts);
    let admin = Visitor::admin();

    let row = flow::view_as(&posts, &as_user(&member, 8), &admin, "3").expect("author 8 sees 3");
    assert_eq!(row.id, 3);

    for identity in [as_user(&member, 7), None] {
        let hidden = flow::view_as(&posts, &identity, &admin, "3").unwrap_err();
        assert!(matches!(hidden, Failure::NoMatch), "{hidden:?}");
        assert_eq!(outcome(Action::View, &hidden).status, 404);
    }
}

/// The admission read and the `Verified` write run in one transaction as
/// the identity, so the database adds its policy to the write.
#[test]
fn an_update_writes_through_the_identity_and_not_without_it() {
    let Some((posts, member)) = rls_posts() else {
        return;
    };
    seed_eight(&posts);
    let admin = Visitor::admin();

    let updated = flow::update_as(
        &posts,
        &as_user(&member, 7),
        &admin,
        "2",
        b"status=published",
    )
    .expect("author 7 updates its own row");
    assert_eq!(updated.row.map(|r| r.status), Some("published".into()));
    assert_eq!(posts.db.status_of(2).as_deref(), Some("published"));

    // Another author's row: the application admits it, the database hides
    // it, so the admission finds nothing.
    let refused = flow::update_as(
        &posts,
        &as_user(&member, 7),
        &admin,
        "4",
        b"status=published",
    )
    .unwrap_err();
    assert!(matches!(refused, Failure::NoMatch), "{refused:?}");
    assert_eq!(posts.db.status_of(4).as_deref(), Some("draft"));

    // No identity: nothing to admit, nothing written.
    let refused = flow::update(&posts, &admin, "1", b"status=draft").unwrap_err();
    assert!(matches!(refused, Failure::NoMatch), "{refused:?}");
    assert_eq!(posts.db.status_of(1).as_deref(), Some("published"));
    assert_eq!(posts.sink.published(), ["posts:2:updated[status]"]);
}

/// The database's `WITH CHECK` refuses a row the identity may not own, in
/// the same transaction: the insert fails and nothing is kept.
#[test]
fn a_create_is_checked_by_the_database_too() {
    let Some((posts, member)) = rls_posts() else {
        return;
    };
    let admin = Visitor::admin();

    let created = flow::create_as(
        &posts,
        &as_user(&member, 7),
        &Visitor::user(7),
        b"title=mine&author_id=7",
    )
    .expect("author 7 creates its own row");
    assert_eq!(created.row.author_id, Some(7));

    // The application's subject is author 8 and admits the row; the
    // database's identity is author 7 and refuses it.
    let refused = flow::create_as(
        &posts,
        &as_user(&member, 7),
        &Visitor::user(8),
        b"title=theirs&author_id=8",
    )
    .unwrap_err();
    assert!(
        refused.to_string().contains("row-level security"),
        "{refused}"
    );

    let deleted = flow::delete_as(
        &posts,
        &as_user(&member, 8),
        &admin,
        &created.id.to_string(),
    )
    .unwrap_err();
    assert!(matches!(deleted, Failure::NoMatch), "{deleted:?}");
    flow::delete_as(
        &posts,
        &as_user(&member, 7),
        &admin,
        &created.id.to_string(),
    )
    .expect("its author deletes it");
    assert_eq!(
        posts.sink.published(),
        ["posts:1:created", "posts:1:deleted"]
    );
}
