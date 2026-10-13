//! A listing is a window over the set the policy admits, and the total counts
//! the same set.
//!
//! PLAN.md §9: *"List authorization is not a per-object property. It is a set
//! comprehension. Any design that ignores this ships a paginator that silently
//! lies."*

mod common;

use brocanth_resource::page::{PageRequest, Totals};
use brocanth_resource::{flow, Failure, Resource};
use common::{row_ids, seed_eight, Posts, Visitor};

#[test]
fn the_policy_reaches_the_where_clause_rather_than_a_filter_afterwards() {
    let posts = Posts::new();
    seed_eight(&posts);

    let listing =
        flow::list(&posts, &Visitor::user(7), &PageRequest::new(), None).expect("a listing");

    // Published posts, or author 7's own. Rows 4, 5, 7 and 8 belong to other
    // authors and are drafts, and they are not here.
    assert_eq!(row_ids(&listing.items), vec![1, 2, 3, 6]);

    // The condition the data source was handed, twice: once to count and once
    // to read. Both are the compiled policy, and this is the assertion that
    // the filtering happened in the query — the fixture is a real database and
    // never saw the other four rows.
    let seen = posts.db.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "one count and one page");
    assert_eq!(seen[0], seen[1], "the total counts what the page reads");
    assert!(
        seen[0].contains("\"posts\".\"status\"") && seen[0].contains("\"posts\".\"author_id\""),
        "the policy's columns are in the condition: {}",
        seen[0]
    );
}

#[test]
fn pages_do_not_overlap_and_the_total_is_the_filtered_count() {
    let posts = Posts::new();
    seed_eight(&posts);
    let who = Visitor::user(7);

    let page = |n: u64| {
        flow::list(
            &posts,
            &who,
            &PageRequest {
                page: n,
                per_page: Some(2),
                skip_total: false,
            },
            None,
        )
        .expect("a listing")
    };

    let first = page(1);
    let second = page(2);
    let third = page(3);

    assert_eq!(row_ids(&first.items), vec![1, 2]);
    assert_eq!(row_ids(&second.items), vec![3, 6]);
    assert!(third.items.is_empty(), "there is no third page");

    // Four admitted rows out of eight. The total is the filtered count, not
    // the table count, and it agrees with the pages that were served.
    for listing in [&first, &second, &third] {
        assert_eq!(listing.total, Totals::Counted(4));
        assert_eq!(listing.total.pages(2), Some(2));
    }
    let served = first.items.len() + second.items.len() + third.items.len();
    assert_eq!(served as u64, listing_total(&first), "every row, once");
}

fn listing_total(listing: &brocanth_resource::page::Listing<common::PostRow>) -> u64 {
    listing.total.count().expect("a counted total")
}

#[test]
fn an_administrator_sees_everything_and_the_total_says_so() {
    let posts = Posts::new();
    seed_eight(&posts);

    let listing =
        flow::list(&posts, &Visitor::admin(), &PageRequest::new(), None).expect("a listing");

    assert_eq!(listing.items.len(), 8);
    assert_eq!(listing.total, Totals::Counted(8));
}

#[test]
fn an_anonymous_visitor_sees_only_what_is_published() {
    let posts = Posts::new();
    seed_eight(&posts);

    let listing =
        flow::list(&posts, &Visitor::anonymous(), &PageRequest::new(), None).expect("a listing");

    assert_eq!(row_ids(&listing.items), vec![1, 3, 6]);
    assert_eq!(listing.total, Totals::Counted(3));
}

#[test]
fn skip_total_skips_the_count_and_reports_no_number_at_all() {
    let posts = Posts::new();
    seed_eight(&posts);

    let listing = flow::list(
        &posts,
        &Visitor::user(7),
        &PageRequest {
            page: 1,
            per_page: Some(2),
            skip_total: true,
        },
        None,
    )
    .expect("a listing");

    assert_eq!(listing.total, Totals::Skipped);
    assert_eq!(listing.total.count(), None, "there is no number to read");
    assert_eq!(listing.total.pages(2), None);
    // One query, not two: the count is the thing `skipTotal` is for.
    assert_eq!(posts.db.seen.lock().unwrap().len(), 1);
}

#[test]
fn per_page_is_clamped_rather_than_refused() {
    let posts = Posts::new();
    seed_eight(&posts);

    let listing = flow::list(
        &posts,
        &Visitor::admin(),
        &PageRequest {
            page: 1,
            per_page: Some(1_000_000),
            skip_total: false,
        },
        None,
    )
    .expect("a listing");

    assert_eq!(listing.per_page, Posts::MAX_PER_PAGE);
}

#[test]
fn a_sort_the_resource_declares_changes_the_order_and_nothing_else() {
    let posts = Posts::new();
    seed_eight(&posts);

    let listing = flow::list(
        &posts,
        &Visitor::user(7),
        &PageRequest::new(),
        Some(brocanth_resource::Sort::descending(common::PostColumn::Id)),
    )
    .expect("a listing");

    assert_eq!(row_ids(&listing.items), vec![6, 3, 2, 1]);
}

#[test]
fn a_list_rule_of_never_is_a_refusal_rather_than_an_empty_page() {
    // `Pred::Never` is the typed form of PocketBase's `null` rule: a fact
    // about the collection and the subject, decided without reading a row, so
    // it is said out loud as a 403. A rule that merely matches nothing is the
    // empty page above, and the two must not be confused.
    let posts = Posts::new();
    seed_eight(&posts);

    let refused = flow::list(&posts, &Visitor::banned(), &PageRequest::new(), None);
    assert!(matches!(refused, Err(Failure::NoRule)));
    assert!(
        posts.db.seen.lock().unwrap().is_empty(),
        "no row was read, and no query was sent"
    );

    let outcome = brocanth_resource::status::outcome(
        brocanth_resource::status::Action::List,
        &Failure::NoRule,
    );
    assert_eq!(outcome.status, 403);
}
