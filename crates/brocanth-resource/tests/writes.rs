//! The write paths, and the three things PLAN.md §9 asks of them: a witness
//! that carries its condition, a lost race that is reported rather than
//! swallowed, and a field mask that is checked before anything is touched.

mod common;

use brocanth_policy::Value;
use brocanth_resource::{flow, Failure};
use common::{seed_eight, Posts, Visitor};

#[test]
fn an_author_updates_a_row_the_policy_admits() {
    let posts = Posts::new();
    seed_eight(&posts);

    let updated =
        flow::update(&posts, &Visitor::user(7), "1", b"title=renamed").expect("the update applies");

    assert_eq!(updated.id, Value::Int(1));
    assert_eq!(updated.changed, vec!["title"]);
    assert_eq!(
        updated.row.expect("the row is still visible").title,
        "renamed"
    );
}

#[test]
fn a_row_that_belongs_to_somebody_else_is_the_same_answer_as_no_row() {
    let posts = Posts::new();
    seed_eight(&posts);

    // Row 3 is author 8's. Row 99 does not exist. One answer for both, which
    // is the point: a different answer would say which rows are there.
    let theirs = flow::update(&posts, &Visitor::user(7), "3", b"title=x");
    let absent = flow::update(&posts, &Visitor::user(7), "99", b"title=x");

    assert!(matches!(theirs, Err(Failure::NoMatch)));
    assert!(matches!(absent, Err(Failure::NoMatch)));
    assert_eq!(
        posts.db.status_of(3).as_deref(),
        Some("published"),
        "and nothing was written"
    );
}

#[test]
fn a_masked_field_is_refused_and_the_row_is_never_touched() {
    let posts = Posts::new();
    seed_eight(&posts);

    // The author may write the title, never the status. PLAN.md §9's
    // `:changed`, as `Policy::writable() -> FieldMask`.
    let refused = flow::update(&posts, &Visitor::user(7), "1", b"status=published");

    match refused {
        Err(Failure::NotWritable(violation)) => {
            assert!(violation.to_string().contains("status"));
        }
        other => panic!("expected a mask violation, got {other:?}"),
    }
    assert_eq!(posts.db.status_of(1).as_deref(), Some("published"));
    assert!(
        posts.db.seen.lock().unwrap().is_empty(),
        "the mask is checked before the row is read: no condition reached the \
         data source, so the 403 cannot be read as `and the row exists`"
    );
}

#[test]
fn a_mask_violation_names_every_rejected_column_at_once() {
    let posts = Posts::new();
    seed_eight(&posts);

    let refused = flow::update(&posts, &Visitor::user(7), "1", b"status=draft&id=4");
    let message = match refused {
        Err(Failure::NotWritable(violation)) => violation.to_string(),
        other => panic!("expected a mask violation, got {other:?}"),
    };
    assert!(
        message.contains("status") && message.contains("id"),
        "{message}"
    );
}

#[test]
fn an_update_that_loses_the_race_is_reported_rather_than_silently_doing_nothing() {
    let posts = Posts::new();
    seed_eight(&posts);

    // Another transaction commits between the admission test and the write:
    // row 1 changes hands. The `UPDATE` carries the policy's condition in its
    // own `WHERE` clause, so it matches nothing — and zero rows affected is an
    // authorisation failure, not a no-op.
    *posts.db.race.lock().unwrap() = Some((
        "UPDATE posts SET author_id = 99 WHERE id = 1".to_owned(),
        Vec::new(),
    ));

    let lost = flow::update(&posts, &Visitor::user(7), "1", b"title=renamed");

    assert!(matches!(lost, Err(Failure::RaceLost)), "{lost:?}");
    assert_eq!(
        brocanth_resource::status::outcome(
            brocanth_resource::status::Action::Update,
            &Failure::RaceLost
        )
        .status,
        409
    );
}

#[test]
fn an_empty_body_is_a_bad_request_rather_than_a_policy_failure() {
    let posts = Posts::new();
    seed_eight(&posts);

    let refused = flow::update(&posts, &Visitor::user(7), "1", b"");
    assert!(
        matches!(refused, Err(Failure::BadRequest(_))),
        "{refused:?}"
    );
}

#[test]
fn a_write_that_moves_the_row_out_of_its_own_view_still_stands() {
    let posts = Posts::new();
    posts.db.seed(&[(1, Some(7), "published", "one")]);

    // The administrator may write the status; the author may not. Writing it
    // to a value the admin's own view predicate still admits keeps the row
    // visible, so the interesting case is the other one: a subject whose view
    // rule stops matching after a legal write.
    let updated =
        flow::update(&posts, &Visitor::admin(), "1", b"status=draft").expect("the update applies");
    assert!(updated.row.is_some(), "an admin sees every row");

    // Author 7 renames their own draft. Still theirs, so still visible.
    let updated =
        flow::update(&posts, &Visitor::user(7), "1", b"title=two").expect("the update applies");
    assert!(updated.row.is_some());

    // Handing it to somebody else is a legal write under this policy, and the
    // row is then no longer author 7's to see. The write stands, and there is
    // nothing to show.
    let updated =
        flow::update(&posts, &Visitor::user(7), "1", b"author_id=8").expect("the update applies");
    assert!(updated.row.is_none(), "the row is no longer visible");
    assert_eq!(posts.db.status_of(1).as_deref(), Some("draft"));
}

#[test]
fn a_delete_rule_of_never_refuses_without_reading_a_row() {
    let posts = Posts::new();
    seed_eight(&posts);

    let refused = flow::delete(&posts, &Visitor::user(7), "1");

    assert!(matches!(refused, Err(Failure::NoRule)));
    assert!(posts.db.seen.lock().unwrap().is_empty());
    assert_eq!(
        brocanth_resource::status::outcome(
            brocanth_resource::status::Action::Delete,
            &Failure::NoRule
        )
        .status,
        403
    );
}

#[test]
fn an_administrator_deletes_and_the_row_is_gone() {
    let posts = Posts::new();
    seed_eight(&posts);

    flow::delete(&posts, &Visitor::admin(), "4").expect("the delete applies");
    assert_eq!(posts.db.status_of(4), None);

    // A second delete of the same row is `NoMatch`, which is the same answer
    // as a row that never existed.
    let again = flow::delete(&posts, &Visitor::admin(), "4");
    assert!(matches!(again, Err(Failure::NoMatch)));
}

#[test]
fn a_delete_that_loses_the_race_is_reported() {
    let posts = Posts::new();
    posts.db.seed(&[(1, Some(7), "published", "one")]);

    // The admin's delete rule is `Always`, which compiles to `1 = 1`, so the
    // only way to lose the race is for the row to leave. That is exactly the
    // case an application reports as "nothing to do".
    *posts.db.race.lock().unwrap() =
        Some(("DELETE FROM posts WHERE id = 1".to_owned(), Vec::new()));

    let lost = flow::delete(&posts, &Visitor::admin(), "1");
    assert!(matches!(lost, Err(Failure::RaceLost)), "{lost:?}");
}

#[test]
fn an_id_that_is_not_a_key_is_a_bad_request() {
    let posts = Posts::new();
    seed_eight(&posts);

    let refused = flow::view(&posts, &Visitor::admin(), "not-a-number");
    assert!(
        matches!(refused, Err(Failure::BadRequest(_))),
        "{refused:?}"
    );
}

#[test]
fn a_view_is_guarded_in_the_same_statement_that_reads() {
    let posts = Posts::new();
    seed_eight(&posts);

    let row = flow::view(&posts, &Visitor::anonymous(), "1").expect("published, so visible");
    assert_eq!(row.id, 1);

    let refused = flow::view(&posts, &Visitor::anonymous(), "2");
    assert!(
        matches!(refused, Err(Failure::NoMatch)),
        "a draft is not public"
    );

    let seen = posts.db.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|sql| sql.contains("\"posts\".\"status\"")));
}
