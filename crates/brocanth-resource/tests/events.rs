//! A change is published when a row changed, and only then.
//!
//! PLAN.md §10 wants realtime to have an input it can invert into an audience.
//! The input is a `RowChange`, and the two things that make it trustworthy are
//! that it is published after the write rather than before, and that it is not
//! built at all when nobody is listening.

mod common;

use brocanth_resource::{flow, Failure};
use common::{seed_eight, Posts, Visitor};

#[test]
fn a_successful_update_publishes_the_row_and_the_columns_it_wrote() {
    let posts = Posts::new();
    seed_eight(&posts);

    flow::update(&posts, &Visitor::user(7), "1", b"title=renamed").expect("the update applies");

    assert_eq!(posts.sink.published(), vec!["posts:1:updated[title]"]);
}

#[test]
fn a_successful_create_and_delete_publish_their_own_kinds() {
    let posts = Posts::new();

    let created = flow::create(&posts, &Visitor::user(7), b"author_id=7&title=new")
        .expect("the create applies");
    flow::delete(&posts, &Visitor::admin(), &created.id.to_string()).expect("the delete applies");

    assert_eq!(
        posts.sink.published(),
        vec!["posts:1:created", "posts:1:deleted"]
    );
}

#[test]
fn a_refused_write_publishes_nothing() {
    let posts = Posts::new();
    seed_eight(&posts);

    // One of each refusal: a rule that is Never, a condition that admits no
    // row, a masked column, and a body that is not usable.
    assert!(matches!(
        flow::delete(&posts, &Visitor::user(7), "1"),
        Err(Failure::NoRule)
    ));
    assert!(matches!(
        flow::update(&posts, &Visitor::user(7), "3", b"title=x"),
        Err(Failure::NoMatch)
    ));
    assert!(matches!(
        flow::update(&posts, &Visitor::user(7), "1", b"status=draft"),
        Err(Failure::NotWritable(_))
    ));
    assert!(matches!(
        flow::update(&posts, &Visitor::user(7), "1", b""),
        Err(Failure::BadRequest(_))
    ));

    assert!(posts.sink.published().is_empty());
}

#[test]
fn a_write_that_lost_the_race_publishes_nothing() {
    let posts = Posts::new();
    posts.db.seed(&[(1, Some(7), "published", "one")]);
    *posts.db.race.lock().unwrap() = Some((
        "UPDATE posts SET author_id = 99 WHERE id = 1".to_owned(),
        Vec::new(),
    ));

    let lost = flow::update(&posts, &Visitor::user(7), "1", b"title=renamed");

    assert!(matches!(lost, Err(Failure::RaceLost)));
    assert!(
        posts.sink.published().is_empty(),
        "the write affected no row, so there is no change to announce"
    );
}

#[test]
fn nothing_is_built_when_nobody_is_listening() {
    let posts = Posts::deaf();
    posts.db.seed(&[(1, Some(7), "published", "one")]);

    flow::update(&posts, &Visitor::user(7), "1", b"title=renamed").expect("the update applies");

    // `ChangeSink::wants` was asked, and because it said no, no change was
    // built: no owned collection name, no field list, no allocation.
    assert_eq!(posts.sink.asked(), 1);
    assert!(posts.sink.published().is_empty());
}
