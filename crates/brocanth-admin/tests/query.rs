//! Reading a list screen's request out of a query string.

use brocanth_admin::{Direction, ListQuery, Route};

#[test]
fn the_five_keys_are_read() {
    let query = ListQuery::parse("?page=3&per_page=50&q=ada&sort=name&dir=desc");

    assert_eq!(query.page, 3);
    assert_eq!(query.per_page, Some(50));
    assert_eq!(query.search, "ada");
    let sort = query.sort.unwrap();
    assert_eq!(sort.column, "name");
    assert_eq!(sort.direction, Direction::Descending);
}

#[test]
fn a_search_term_survives_a_round_trip() {
    let query = ListQuery::parse("q=caf%C3%A9+%26+bar&sort=name&dir=asc");
    assert_eq!(query.search, "café & bar");

    let again = ListQuery::parse(&query.to_query_string(2));
    assert_eq!(again.search, "café & bar");
    assert_eq!(again.page, 2);
    assert_eq!(again.sort.unwrap().column, "name");
}

#[test]
fn a_percent_in_front_of_a_multibyte_character_does_not_panic() {
    // `%` then the two first bytes of a three-byte character: slicing the
    // `&str` here would split a char and panic.
    assert_eq!(ListQuery::parse("q=%€").search, "%€");
    assert_eq!(ListQuery::parse("q=100%").search, "100%");
    assert_eq!(ListQuery::parse("q=%zz").search, "%zz");
    assert_eq!(ListQuery::parse("q=%2").search, "%2");
}

#[test]
fn a_nonsense_query_string_still_yields_a_list() {
    let query = ListQuery::parse("page=banana&per_page=&dir=sideways&unknown=1&&=");

    assert_eq!(query.page, 1, "not an error, and not page zero");
    assert_eq!(query.per_page, None);
    assert!(query.sort.is_none(), "a direction without a column is no sort");
}

#[test]
fn a_path_outside_the_mount_is_not_this_admins() {
    assert_eq!(Route::parse("/admin", "GET", "/other/post"), None);
    assert_eq!(Route::parse("/admin", "GET", "/administrator"), None);
    assert_eq!(Route::parse("/admin", "GET", "/admin"), Some(Route::Index));
    assert_eq!(
        Route::parse("/admin", "GET", "/admin/post"),
        Some(Route::List("post"))
    );
    // Mounted at the root, everything is in scope.
    assert_eq!(Route::parse("", "GET", "/post"), Some(Route::List("post")));
}
