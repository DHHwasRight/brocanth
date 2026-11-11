//! A keyset walk visits every row once, in order, whatever the ties.
//!
//! The predicate is checked with `Pred::eval`, the in-memory reading of the
//! same `Pred` the database gets. `brocanth-policy`'s agreement tests prove
//! the two readings agree; this file proves the predicate is the right one.

use brocanth_pagination::{Cursor, Direction, Key, Keyset, KeysetPage};
use brocanth_policy::{Column, Dialect, Entity, Ident, MapRow, Pred, SortKind, Sortable};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PostColumn {
    Id,
    Score,
}

impl Column for PostColumn {
    const ALL: &'static [Self] = &[Self::Id, Self::Score];

    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Score => "score",
        }
    }
}

impl Sortable for PostColumn {
    fn sort_kind(&self) -> Option<SortKind> {
        Some(SortKind::Int)
    }
}

struct Post;

impl Entity for Post {
    type Column = PostColumn;
    const TABLE: Ident = Ident::must("posts");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}

/// `(id, score)`, with many ties on score.
fn rows() -> Vec<(i64, i64)> {
    (1..=23).map(|id| (id, (id * 7) % 5)).collect()
}

fn row(id: i64, score: i64) -> MapRow<PostColumn> {
    MapRow::new()
        .with(PostColumn::Id, id)
        .with(PostColumn::Score, score)
}

/// Reads one page the way a database would: the predicate, the order, and
/// one row more than the page.
fn read(
    keyset: &Keyset<PostColumn>,
    direction: Direction,
    after: Option<&Cursor>,
    limit: u64,
) -> KeysetPage<(i64, i64)> {
    let pred: Pred<PostColumn> = keyset.after(after).expect("a cursor of the right width");
    let mut admitted: Vec<(i64, i64)> = rows()
        .into_iter()
        .filter(|(id, score)| pred.admits(&row(*id, *score)).expect("no opaque part"))
        .collect();
    admitted.sort_by_key(|(id, score)| (*score, *id));
    if direction == Direction::Descending {
        admitted.reverse();
    }
    admitted.truncate(usize::try_from(Keyset::<PostColumn>::fetch(limit)).unwrap());
    KeysetPage::from_rows(admitted, limit, |(id, score)| {
        Cursor::new([Key::Int(*score), Key::Int(*id)])
    })
}

fn walk(direction: Direction, limit: u64) -> Vec<(i64, i64)> {
    let keyset = Keyset::new(PostColumn::Score, PostColumn::Id, direction);
    let mut seen = Vec::new();
    let mut after: Option<Cursor> = None;
    loop {
        let page = read(&keyset, direction, after.as_ref(), limit);
        seen.extend(page.items);
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    seen
}

/// The rows `pred` admits, sorted by `keyset`'s order in `direction`, and
/// cut to `Keyset::fetch(limit)`: what the database returns.
fn select(pred: &Pred<PostColumn>, direction: Direction, limit: u64) -> Vec<(i64, i64)> {
    let mut admitted: Vec<(i64, i64)> = rows()
        .into_iter()
        .filter(|(id, score)| pred.admits(&row(*id, *score)).expect("no opaque part"))
        .collect();
    admitted.sort_by_key(|(id, score)| (*score, *id));
    if direction == Direction::Descending {
        admitted.reverse();
    }
    admitted.truncate(usize::try_from(Keyset::<PostColumn>::fetch(limit)).unwrap());
    admitted
}

fn key(row: &(i64, i64)) -> Cursor {
    Cursor::new([Key::Int(row.1), Key::Int(row.0)])
}

/// Every page, forward from the first, as a handler reads them: the first
/// with no cursor, each next one `after` the last page's `next`.
fn forward(
    keyset: &Keyset<PostColumn>,
    direction: Direction,
    limit: u64,
) -> Vec<KeysetPage<(i64, i64)>> {
    let first = KeysetPage::from_rows(
        select(&keyset.after(None).unwrap(), direction, limit),
        limit,
        key,
    );
    let mut pages = vec![first];
    while let Some(next) = pages.last().unwrap().next.clone() {
        let rows = select(&keyset.after(Some(&next)).unwrap(), direction, limit);
        pages.push(KeysetPage::after(rows, limit, key));
    }
    pages
}

/// The page before `cursor`: read in the reversed order, then put back.
fn backward(
    keyset: &Keyset<PostColumn>,
    direction: Direction,
    cursor: &Cursor,
    limit: u64,
) -> KeysetPage<(i64, i64)> {
    let reversed = match direction {
        Direction::Ascending => Direction::Descending,
        Direction::Descending => Direction::Ascending,
    };
    assert_eq!(
        keyset.reversed(),
        Keyset::new(PostColumn::Score, PostColumn::Id, reversed)
    );
    let rows = select(&keyset.before(cursor).unwrap(), reversed, limit);
    KeysetPage::before(rows, limit, key)
}

#[test]
fn a_previous_link_reads_back_the_same_pages_through_the_ties() {
    // Scores 0 to 4, each held by four or five rows: every page edge for
    // these limits falls inside a tie somewhere.
    for direction in [Direction::Ascending, Direction::Descending] {
        let keyset = Keyset::new(PostColumn::Score, PostColumn::Id, direction);
        for limit in [1, 2, 3, 4, 5, 7, 22, 23, 50] {
            let pages = forward(&keyset, direction, limit);
            let label = format!("{direction:?}, limit {limit}");

            // First page: no previous link. Last page: no next link.
            assert_eq!(pages[0].previous, None, "{label}");
            assert_eq!(pages.last().unwrap().next, None, "{label}");
            // Every page after the first links back.
            for page in &pages[1..] {
                assert!(page.previous.is_some(), "{label}");
            }

            // Walk back from the last page: each previous page is the page
            // that came before it going forward, with the same links.
            for index in (1..pages.len()).rev() {
                let cursor = pages[index].previous.as_ref().unwrap();
                let back = backward(&keyset, direction, cursor, limit);
                assert_eq!(back.items, pages[index - 1].items, "{label}, page {index}");
                assert_eq!(back.next, Some(key(pages[index - 1].items.last().unwrap())));
                if index == 1 {
                    assert_eq!(back.previous, None, "{label}: back on the first page");
                } else {
                    assert_eq!(
                        back.previous,
                        pages[index - 1].previous,
                        "{label}, page {index}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_cursor_on_the_first_row_has_nothing_before_it() {
    let keyset = Keyset::new(PostColumn::Score, PostColumn::Id, Direction::Ascending);
    // (5, 0) is the first row: score 0, the lowest id among the zeros.
    let first_row = Cursor::new([Key::Int(0), Key::Int(5)]);
    let back = backward(&keyset, Direction::Ascending, &first_row, 3);
    assert!(back.items.is_empty());
    assert_eq!((back.previous, back.next), (None, None));
    // A mistyped cursor is refused going back too.
    assert!(keyset.before(&Cursor::parse("t61.i1").unwrap()).is_err());
}

#[test]
fn every_row_once_in_either_direction() {
    for limit in [1, 2, 4, 5, 23, 50] {
        let mut expected = rows();
        expected.sort_by_key(|(id, score)| (*score, *id));
        assert_eq!(walk(Direction::Ascending, limit), expected, "limit {limit}");
        expected.reverse();
        assert_eq!(
            walk(Direction::Descending, limit),
            expected,
            "limit {limit}"
        );
    }
}

#[test]
fn the_cursor_compiles_into_the_where_beside_the_rule() {
    let keyset = Keyset::new(PostColumn::Score, PostColumn::Id, Direction::Descending);
    let cursor = Cursor::new([Key::Int(3), Key::Int(9)]);
    let rule = Pred::eq(PostColumn::Score, 3_i64);
    let pred = rule.and(keyset.after(Some(&cursor)).unwrap());
    let condition = pred.condition::<Post>(Dialect::Postgres).unwrap();
    assert_eq!(
        condition.sql(),
        "(\"posts\".\"score\" = $1) AND ((\"posts\".\"score\" < $2) OR ((\"posts\".\"score\" = $3) AND (\"posts\".\"id\" < $4)))"
    );
    assert_eq!(
        keyset.order_by::<Post>().unwrap(),
        "\"posts\".\"score\" DESC, \"posts\".\"id\" DESC"
    );
}

#[test]
fn a_key_only_order_needs_a_one_key_cursor() {
    let keyset = Keyset::by_key(PostColumn::Id, Direction::Ascending);
    assert_eq!(keyset.width(), 1);
    assert_eq!(keyset.after(None).unwrap(), Pred::Always);
    assert_eq!(
        keyset.after(Some(&Cursor::new([Key::Int(4)]))).unwrap(),
        Pred::gt(PostColumn::Id, 4_i64)
    );
    let wrong = keyset
        .after(Some(&Cursor::new([Key::Int(1), Key::Int(2)])))
        .unwrap_err();
    assert!(wrong.to_string().contains("help:"));
    assert_eq!(keyset.order_by::<Post>().unwrap(), "\"posts\".\"id\" ASC");
}

/// Reads one page inside `rule`, as a handler does: the rule AND the cursor.
fn read_within(
    rule: &Pred<PostColumn>,
    keyset: &Keyset<PostColumn>,
    after: Option<&Cursor>,
    limit: u64,
) -> Vec<(i64, i64)> {
    let pred = rule.clone().and(keyset.after(after).expect("right width"));
    let mut admitted: Vec<(i64, i64)> = rows()
        .into_iter()
        .filter(|(id, score)| pred.admits(&row(*id, *score)).expect("no opaque part"))
        .collect();
    admitted.sort_by_key(|(id, score)| (*score, *id));
    admitted.truncate(usize::try_from(limit).unwrap());
    admitted
}

#[test]
fn a_forged_cursor_moves_the_window_and_never_passes_the_rule() {
    // The rule admits scores 1 and 3 only. A client can write any cursor;
    // each one only picks where the window starts.
    let rule = Pred::one_of(PostColumn::Score, [1_i64, 3]);
    let keyset = Keyset::new(PostColumn::Score, PostColumn::Id, Direction::Ascending);
    for score in [i64::MIN, -1, 0, 1, 2, 3, 4, 99, i64::MAX] {
        for id in [i64::MIN, 0, 5, 12, i64::MAX] {
            let forged = Cursor::new([Key::Int(score), Key::Int(id)]);
            for (row_id, row_score) in read_within(&rule, &keyset, Some(&forged), 100) {
                assert!(
                    row_score == 1 || row_score == 3,
                    "cursor ({score}, {id}) reached score {row_score}"
                );
                assert!(
                    (row_score, row_id) > (score, id),
                    "cursor ({score}, {id}) returned ({row_score}, {row_id}), before it"
                );
            }
        }
    }
    let past = Cursor::new([Key::Int(i64::MAX), Key::Int(i64::MAX)]);
    assert!(read_within(&rule, &keyset, Some(&past), 100).is_empty());
}

#[test]
fn ties_on_the_sort_column_are_split_by_the_key_at_the_page_edge() {
    // Five rows share score 2 (ids 1, 6, 11, 16, 21). A page that ends
    // inside the tie starts the next page at the next id, not the next
    // score.
    let keyset = Keyset::new(PostColumn::Score, PostColumn::Id, Direction::Ascending);
    let cursor = Cursor::new([Key::Int(2), Key::Int(6)]);
    let next = read_within(&Pred::Always, &keyset, Some(&cursor), 3);
    assert_eq!(next, [(11, 2), (16, 2), (21, 2)]);

    let descending = Keyset::new(PostColumn::Score, PostColumn::Id, Direction::Descending);
    let pred = descending.after(Some(&cursor)).unwrap();
    assert!(pred.admits(&row(1, 2)).unwrap(), "a lower id in the tie");
    assert!(!pred.admits(&row(11, 2)).unwrap(), "a higher id in the tie");
    assert!(!pred.admits(&row(6, 2)).unwrap(), "the cursor row itself");
}

#[test]
fn a_cursor_from_the_query_string_of_the_wrong_width_is_a_help_line() {
    use brocanth_pagination::Params;
    let keyset = Keyset::new(PostColumn::Score, PostColumn::Id, Direction::Ascending);
    let params = Params::parse("after=i3").unwrap();
    let error = keyset.after(params.after.as_ref()).unwrap_err();
    assert!(error.to_string().contains("2 key(s)"));
    assert!(error.to_string().contains("help:"));
    let params = Params::parse("after=i3.i4.i5").unwrap();
    assert!(keyset.after(params.after.as_ref()).is_err());
}

#[test]
fn a_cursor_key_of_the_wrong_kind_is_not_a_cursor() {
    // Text where the score is an integer, and an integer where the id is:
    // each one would reach the database as `bigint > text` and fail there.
    let keyset = Keyset::new(PostColumn::Score, PostColumn::Id, Direction::Ascending);
    for token in ["t61.i1", "i1.t61", "t61.t62"] {
        let cursor = Cursor::parse(token).unwrap();
        let error = keyset.after(Some(&cursor)).unwrap_err();
        assert!(
            error.to_string().contains("is not a cursor"),
            "{token}: {error}"
        );
        assert!(error.to_string().contains("help:"), "{token}");
    }
    assert!(keyset.after(Some(&Cursor::parse("i1.i1").unwrap())).is_ok());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PriceColumn {
    Id,
    Price,
}

impl Column for PriceColumn {
    const ALL: &'static [Self] = &[Self::Id, Self::Price];

    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Price => "price",
        }
    }
}

impl Sortable for PriceColumn {
    fn sort_kind(&self) -> Option<SortKind> {
        match self {
            Self::Id => Some(SortKind::Int),
            Self::Price => None,
        }
    }
}

#[test]
fn an_order_on_a_column_a_cursor_cannot_carry_fails_on_the_first_page() {
    let keyset = Keyset::new(PriceColumn::Price, PriceColumn::Id, Direction::Ascending);
    let error = keyset.after(None).unwrap_err();
    assert!(error.to_string().contains("`price`"), "{error}");
    assert!(error.to_string().contains("help:"), "{error}");
    assert!(Keyset::by_key(PriceColumn::Id, Direction::Ascending)
        .after(None)
        .is_ok());
}
