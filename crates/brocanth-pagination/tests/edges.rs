//! The values a client can type into a query string: a page of zero, a
//! negative page, a number too big for any type, a cursor written by hand.
//! Each one is a page, or an error with a `help:` line. None is a panic.

use brocanth_pagination::{
    query, Cursor, Key, KeysetPage, KeysetPager, Limits, Links, Page, Pager, ParamError, Params,
};

#[test]
fn page_zero_and_an_empty_page_are_page_one() {
    for query in ["page=0", "page=", "page", "", "?page=1"] {
        assert_eq!(Params::parse(query).unwrap().page, 1, "{query}");
    }
}

#[test]
fn a_negative_or_fractional_page_is_an_error_with_a_help_line() {
    for query in [
        "page=-1",
        "page=1.5",
        "page=1e3",
        "page=%20",
        "per_page=-10",
        "per_page=ten",
    ] {
        let error = Params::parse(query).expect_err(query);
        assert!(
            matches!(error, ParamError::NotANumber { .. }),
            "{query}: {error:?}"
        );
        assert!(error.to_string().contains("help:"), "{query}");
    }
}

#[test]
fn a_number_too_big_for_u64_is_an_error_and_u64_max_is_clamped() {
    let error = Params::parse("page=18446744073709551616").unwrap_err();
    assert!(error.to_string().contains("help:"));

    let params = Params::parse("page=18446744073709551615&per_page=18446744073709551615").unwrap();
    assert_eq!(params.page, u64::MAX);
    let limits = Limits::default();
    let page = limits.page(params.page, params.per_page, 95);
    assert_eq!(page.per_page(), 100, "the ceiling applies");
    assert_eq!(page.number(), 1, "past the end is the last page");
    assert_eq!(limits.window(u64::MAX, params.per_page).1, 100);
    assert_eq!(
        limits.window(u64::MAX, None).0,
        u64::MAX,
        "the offset saturates"
    );
}

#[test]
fn the_first_value_of_a_repeated_key_wins() {
    let params = Params::parse("page=2&page=nope&per_page=5&per_page=7").unwrap();
    assert_eq!(params.page, 2);
    assert_eq!(params.per_page, Some(5));
}

#[test]
fn a_page_past_the_end_reads_the_last_page() {
    let limits = Limits::new(10, 50);
    let page = limits.page(1_000_000, None, 31);
    assert_eq!(page.number(), 4);
    assert_eq!(page.offset(), 30);
    assert_eq!(page.range(), Some((31, 31)));
    assert_eq!(page.next(), None);
}

#[test]
fn per_page_limits_hold_at_every_edge() {
    let limits = Limits::new(0, 0);
    assert_eq!(limits.default, 1, "a default of zero is one");
    assert_eq!(limits.max, 1);
    assert_eq!(limits.per_page(Some(500)), 1);

    let limits = Limits::new(20, 50);
    assert_eq!(limits.per_page(Some(50)), 50);
    assert_eq!(limits.per_page(Some(51)), 50);
    assert_eq!(limits.per_page(Some(1)), 1);
    assert_eq!(limits.per_page(None), 20);
    assert_eq!(Limits::default(), Limits::new(25, 100));
}

#[test]
fn huge_page_values_do_not_overflow() {
    let page = Page::new(u64::MAX, u64::MAX, u64::MAX);
    assert_eq!(page.number(), 1);
    assert_eq!(page.range(), Some((1, u64::MAX)));

    // Two pages, each more than half of u64: the end of page two is past
    // u64::MAX before it is clamped to the total.
    let half = u64::MAX / 2 + 1;
    let page = Page::new(2, half, u64::MAX);
    assert_eq!(page.total_pages(), 2);
    assert_eq!(page.range(), Some((half + 1, u64::MAX)));
    assert!(page.summary().contains("page 2 of 2"));

    assert_eq!(Page::new(3, 1, 5).window(0), vec![3], "a span of zero");
    assert_eq!(
        Page::new(u64::MAX, 1, u64::MAX).window(3),
        vec![u64::MAX - 2, u64::MAX - 1, u64::MAX]
    );
}

#[test]
fn a_cursor_with_a_multibyte_character_is_refused_not_a_panic() {
    // `1é1` is four bytes, an even count, and the second hex pair starts
    // inside `é`. Slicing the text at byte 2 panics; reading bytes refuses.
    for token in ["t1é1", "té", "té1", "tébc", "ié", "é", "t日本", "i1.t1é1"] {
        let error = Cursor::parse(token).expect_err(token);
        assert!(error.to_string().contains("help:"), "{token}");
    }
    let error = Params::parse("after=t1%C3%A91").unwrap_err();
    assert!(matches!(error, ParamError::Cursor(_)));
}

#[test]
fn a_tampered_cursor_is_refused_with_a_help_line() {
    for token in [
        "i9223372036854775808",
        "i-9223372036854775809",
        "i1.",
        ".i1",
        "I1",
        "T41",
        "t4",
        "tgg",
        "i1;DROP TABLE posts",
        "i 1",
        "tc328", // the bytes of `(`, but not UTF-8 alone
        "tff",
    ] {
        let error = Cursor::parse(token).expect_err(token);
        assert!(error.to_string().contains("help:"), "{token}");
    }
    assert_eq!(
        Cursor::parse("i-9223372036854775808").unwrap(),
        Cursor::new([Key::Int(i64::MIN)])
    );
    assert_eq!(
        Cursor::parse("t").unwrap(),
        Cursor::new([Key::Text(String::new())])
    );
    assert_eq!(
        Cursor::parse("t4F4b").unwrap(),
        Cursor::new([Key::from("OK")]),
        "uppercase hex reads the same bytes"
    );
}

#[test]
fn a_cursor_error_does_not_echo_markup_unescaped_into_html_by_itself() {
    // The error text names the token. A page that renders it must escape it,
    // and the token a URL carries can hold anything; check it is kept
    // verbatim so the escaping is the page's single job.
    let error = Cursor::parse("<script>").unwrap_err().to_string();
    assert!(error.contains("`<script>`"));
}

#[test]
fn every_text_and_int_round_trips_through_a_link() {
    let keys = [
        Key::Text(String::new()),
        Key::Text("ünïcødé 日本 🎉".into()),
        Key::Text("a.b-c\"'<>&=?#%+ ".into()),
        Key::Text("\0\n\t".into()),
        Key::Int(i64::MIN),
        Key::Int(i64::MAX),
        Key::Int(0),
    ];
    for key in keys {
        let cursor = Cursor::new([key.clone(), Key::Int(7)]);
        let link = Links::new("/feed", "q=x").after(&cursor);
        let query_string = link.split_once('?').unwrap().1;
        let params = Params::parse(query_string).unwrap();
        assert_eq!(params.after, Some(cursor), "{key:?}");
    }
}

#[test]
fn links_keep_per_page_and_drop_every_page_and_after() {
    let links = Links::new("/posts", "page=2&per_page=10&page=3&after=i1&p%61ge=9&q=x");
    assert_eq!(links.page(5), "/posts?per_page=10&q=x&page=5");
    assert_eq!(links.first(), "/posts?per_page=10&q=x");
    assert_eq!(links.page(0), links.first(), "page zero links to page one");
}

#[test]
fn links_escape_what_they_keep() {
    let links = Links::new(
        "/posts",
        "q=%22%3E%3Cscript%3Ealert(1)%3C%2Fscript%3E&a%26b=c%3Dd&tag=caf%C3%A9&x=1+2",
    );
    let href = links.page(2);
    assert!(!href.contains('<') && !href.contains('>') && !href.contains('"'));
    assert!(!href.contains(' '));
    assert_eq!(
        href,
        "/posts?q=%22%3E%3Cscript%3Ealert%281%29%3C%2Fscript%3E&a%26b=c%3Dd&tag=caf%C3%A9&x=1%202&page=2"
    );
    // The link reads back to the same pairs.
    let (_, query_string) = href.split_once('?').unwrap();
    assert_eq!(
        query::pairs(query_string),
        vec![
            ("q".into(), "\"><script>alert(1)</script>".into()),
            ("a&b".into(), "c=d".into()),
            ("tag".into(), "café".into()),
            ("x".into(), "1 2".into()),
            ("page".into(), "2".into()),
        ]
    );
}

#[test]
fn a_malformed_query_string_keeps_its_meaning() {
    let links = Links::new("/p", "&&q=50%&=v&flag&bad=%ff");
    assert_eq!(links.first(), "/p?q=50%25&=v&flag=&bad=%EF%BF%BD");
}

#[test]
fn a_pager_with_one_page_is_not_needed_and_has_no_links() {
    let pager = Pager::new(Page::new(1, 10, 0), &Links::new("/p", "q=x"));
    assert!(!pager.is_needed());
    assert_eq!(pager.previous, None);
    assert_eq!(pager.next, None);
    assert_eq!(pager.summary(), "Nothing to show");
    assert_eq!(pager.pages.len(), 1);
}

#[test]
fn a_pager_past_the_end_marks_the_last_page() {
    let page = Limits::new(10, 10).page(99, None, 25);
    let pager = Pager::new(page, &Links::new("/p", "page=99"));
    assert_eq!(pager.next, None);
    assert_eq!(pager.previous.as_deref(), Some("/p?page=2"));
    assert_eq!(pager.pages.iter().filter(|l| l.current).count(), 1);
    assert!(pager.pages.last().unwrap().current);
}

#[test]
fn a_keyset_page_with_a_limit_of_zero_shows_one_row() {
    let page = KeysetPage::from_rows(vec![1, 2], 0, |n| Cursor::new([Key::Int(*n)]));
    assert_eq!(page.items, vec![1]);
    assert_eq!(page.next, Some(Cursor::new([Key::Int(1)])));
    let empty = KeysetPage::<i64>::from_rows(Vec::new(), 10, |n| Cursor::new([Key::Int(*n)]));
    assert!(empty.items.is_empty());
    assert_eq!(empty.next, None);
}

#[test]
fn a_keyset_pager_on_a_middle_page_links_both_ways() {
    let links = Links::new("/feed", "after=i3&tag=a%20b");
    let pager = KeysetPager::new(
        &links,
        Some(&Cursor::new([Key::Int(3)])),
        Some(&Cursor::new([Key::Int(6)])),
    );
    assert_eq!(pager.first.as_deref(), Some("/feed?tag=a%20b"));
    assert_eq!(pager.next.as_deref(), Some("/feed?tag=a%20b&after=i6"));
}
