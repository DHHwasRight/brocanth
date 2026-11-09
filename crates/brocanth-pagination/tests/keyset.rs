//! A keyset walk visits every row once, in order, whatever the ties.
//!
//! The predicate is checked with `Pred::eval`, the in-memory reading of the
//! same `Pred` the database gets. `brocanth-policy`'s agreement tests prove
//! the two readings agree; this file proves the predicate is the right one.

use brocanth_pagination::{Cursor, Direction, Key, Keyset, KeysetPage};
use brocanth_policy::{Column, Dialect, Entity, Ident, MapRow, Pred};

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
