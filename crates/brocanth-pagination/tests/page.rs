//! Offset arithmetic at the four boundaries that are always wrong. These were
//! the admin's tests when the admin had a paginator of its own.

use brocanth_pagination::{Limits, Page};

#[test]
fn limits_apply_the_ceiling_rather_than_refuse() {
    let limits = Limits::new(25, 100);
    assert_eq!(limits.per_page(None), 25);
    assert_eq!(limits.per_page(Some(1_000_000)), 100);
    assert_eq!(limits.per_page(Some(0)), 1);
    assert_eq!(limits.window(3, Some(10)), (20, 10));
    assert_eq!(limits.window(0, None), (0, 25), "page zero is page one");
    assert_eq!(
        Limits::new(50, 10).max,
        50,
        "a ceiling below the default is raised"
    );
}

#[test]
fn the_first_page_has_no_previous() {
    let page = Page::new(1, 10, 95);

    assert_eq!(page.number(), 1);
    assert_eq!(page.offset(), 0);
    assert_eq!(page.limit(), 10);
    assert_eq!(page.total_pages(), 10);
    assert!(page.is_first());
    assert!(!page.is_last());
    assert_eq!(page.previous(), None);
    assert_eq!(page.next(), Some(2));
    assert_eq!(page.range(), Some((1, 10)));
}

#[test]
fn the_last_page_has_no_next_and_is_short() {
    let page = Page::new(10, 10, 95);

    assert!(page.is_last());
    assert_eq!(page.next(), None);
    assert_eq!(page.previous(), Some(9));
    assert_eq!(page.offset(), 90);
    assert_eq!(
        page.range(),
        Some((91, 95)),
        "the last page holds five rows"
    );
}

#[test]
fn an_exact_multiple_does_not_add_an_empty_page() {
    let page = Page::new(1, 10, 100);

    assert_eq!(page.total_pages(), 10, "not 11");
    assert_eq!(Page::new(10, 10, 100).range(), Some((91, 100)));
    assert!(Page::new(10, 10, 100).is_last());
}

#[test]
fn an_empty_table_is_one_page_that_says_so() {
    let page = Page::new(1, 25, 0);

    assert_eq!(page.total_pages(), 1, "never zero");
    assert_eq!(page.number(), 1);
    assert!(page.is_first());
    assert!(page.is_last());
    assert!(page.is_empty());
    assert_eq!(page.range(), None);
    assert_eq!(page.offset(), 0);
}

#[test]
fn a_page_past_the_end_is_clamped_rather_than_served_empty() {
    let page = Page::new(99, 10, 95);

    assert_eq!(page.number(), 10);
    assert_eq!(page.range(), Some((91, 95)));
}

#[test]
fn page_zero_is_page_one() {
    assert_eq!(Page::new(0, 10, 95).number(), 1);
}

#[test]
fn a_page_size_of_zero_cannot_divide_by_zero() {
    let page = Page::new(1, 0, 3);

    assert_eq!(page.per_page(), 1);
    assert_eq!(page.total_pages(), 3);
}

#[test]
fn one_row_fewer_than_a_page_is_still_one_page() {
    let page = Page::new(1, 10, 9);

    assert_eq!(page.total_pages(), 1);
    assert!(page.is_last());
    assert_eq!(page.range(), Some((1, 9)));
}

#[test]
fn one_row_more_than_a_page_is_two() {
    let page = Page::new(2, 10, 11);

    assert_eq!(page.total_pages(), 2);
    assert_eq!(page.range(), Some((11, 11)));
}

#[test]
fn the_window_stays_inside_the_pages_that_exist() {
    assert_eq!(Page::new(1, 10, 30).window(7), vec![1, 2, 3]);
    assert_eq!(Page::new(1, 1, 100).window(5), vec![1, 2, 3, 4, 5]);
    assert_eq!(Page::new(100, 1, 100).window(5), vec![96, 97, 98, 99, 100]);
    assert_eq!(Page::new(50, 1, 100).window(5), vec![48, 49, 50, 51, 52]);
}
