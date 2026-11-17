//! The `uuid` and `timestamptz` kinds (docs/policy-rls-bridge.md, decision 5).
//!
//! A uuid compared as text puts a cast on the column, and PostgreSQL then
//! cannot use the column's index. So each kind is a [`Value`] of its own, a
//! column type with its own [`typed::Kind`], and a bind PostgreSQL types as
//! `uuid` or `timestamptz`.

#[path = "common/postgres.rs"]
mod postgres;

use brocanth_policy::agreement::check;
use brocanth_policy::typed::{self, ColumnType, Kind};
use brocanth_policy::{Bound, Column, Dialect, Entity, Ident, MapRow, Missing, Pred, Value};
use chrono::{DateTime, TimeZone as _, Utc};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum C {
    Id,
    Owner,
    At,
}

impl Column for C {
    const ALL: &'static [Self] = &[Self::Id, Self::Owner, Self::At];
    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Owner => "owner",
            Self::At => "at",
        }
    }
}

struct Event;

impl Entity for Event {
    type Column = C;
    const TABLE: Ident = Ident::must("events");
    const PRIMARY_KEY: C = C::Id;
}

const PG: Dialect = Dialect::Postgres;

fn owner(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0).single().expect("a valid time")
}

#[test]
fn a_uuid_is_its_sixteen_bytes() {
    let id = owner(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
    let value = Value::from(id);
    assert_eq!(value, Value::Uuid(*id.as_bytes()));
    assert_eq!(value.kind(), "uuid");
    assert_eq!(value.to_string(), id.hyphenated().to_string());
    assert_eq!(Bound::from(id), Bound::of(Value::Uuid(id.into_bytes())));
}

#[test]
fn a_timestamp_is_microseconds_since_the_epoch() {
    let t = Utc
        .timestamp_opt(1_700_000_000, 123_456_789)
        .single()
        .expect("a valid time");
    let value = Value::from(t);
    // Truncated to PostgreSQL's resolution.
    assert_eq!(value, Value::Timestamptz(1_700_000_000_123_456));
    assert_eq!(value.kind(), "timestamptz");
    assert_eq!(value.to_string(), "2023-11-14T22:13:20.123456Z");
    assert_eq!(
        Value::Timestamptz(-1).to_string(),
        "1969-12-31T23:59:59.999999Z"
    );
    assert_eq!(
        Value::Timestamptz(0).to_string(),
        "1970-01-01T00:00:00.000000Z"
    );
}

#[test]
fn the_column_types_have_their_own_kinds() {
    assert_eq!(<<Uuid as ColumnType>::Kind as Kind>::NAME, "uuid");
    assert_eq!(
        <<DateTime<Utc> as ColumnType>::Kind as Kind>::NAME,
        "timestamptz"
    );
    const { assert!(!<Uuid as ColumnType>::NULLABLE) };
    const { assert!(<Option<Uuid> as ColumnType>::NULLABLE) };
    assert!(<Option<DateTime<Utc>> as ColumnType>::samples().contains(&Value::Null));
    assert!(<Uuid as ColumnType>::samples()
        .iter()
        .all(|v| matches!(v, Value::Uuid(_))));
    assert!(<DateTime<Utc> as ColumnType>::samples()
        .iter()
        .all(|v| matches!(v, Value::Timestamptz(_))));
    // A key per row, each one different.
    assert_ne!(<Uuid as ColumnType>::key(1), <Uuid as ColumnType>::key(2));
    // No keyset cursor carries either kind yet.
    assert_eq!(<typed::Uuid as Kind>::SORT, None);
    assert_eq!(<typed::Timestamptz as Kind>::SORT, None);
}

#[test]
fn typed_bind_takes_the_value_and_its_reference() {
    let id = owner(7);
    assert_eq!(typed::bind::<typed::Uuid, _>(id), Bound::from(id));
    assert_eq!(typed::bind::<typed::Uuid, _>(&id), Bound::from(id));
    assert_eq!(
        typed::bind::<typed::Timestamptz, _>(at(5)),
        Bound::of(Value::Timestamptz(5_000_000))
    );
    assert_eq!(
        typed::absent::<typed::Uuid, Uuid>(None, Missing::Deny),
        Bound::or_deny(None::<Uuid>)
    );
    assert_eq!(
        typed::set::<typed::Uuid, _>([owner(1), owner(2)]),
        vec![Value::from(owner(1)), Value::from(owner(2))]
    );
}

#[test]
fn a_condition_binds_each_kind_as_a_parameter() {
    let pred = Pred::all([Pred::eq(C::Owner, owner(7)), Pred::lt(C::At, at(10))]);
    let condition = pred.condition::<Event>(PG).expect("compiles");
    assert_eq!(
        condition.sql(),
        r#"("events"."owner" = $1) AND ("events"."at" < $2)"#
    );
    assert_eq!(
        condition.binds(),
        &[Value::from(owner(7)), Value::Timestamptz(10_000_000)]
    );
}

#[test]
fn eval_orders_uuids_by_bytes_and_timestamps_by_time() {
    let row = MapRow::new()
        .with(C::Id, 1i64)
        .with(C::Owner, owner(5))
        .with(C::At, at(100));
    let admits = |pred: Pred<C>| pred.admits(&row).expect("evaluates");
    assert!(admits(Pred::eq(C::Owner, owner(5))));
    assert!(!admits(Pred::eq(C::Owner, owner(6))));
    assert!(admits(Pred::lt(C::Owner, owner(6))));
    assert!(admits(Pred::gt(C::At, at(99))));
    assert!(!admits(Pred::gt(C::At, at(100))));
    assert!(admits(Pred::one_of(C::Owner, [owner(1), owner(5)])));
}

/// The two interpretations agree on PostgreSQL's own `uuid` and
/// `timestamptz`, NULLs and near misses included.
#[test]
fn eval_agrees_with_postgres_on_uuid_and_timestamptz() {
    let Some(mut db) = postgres::Postgres::new(
        "CREATE TABLE events (id BIGINT PRIMARY KEY, owner UUID, at TIMESTAMPTZ)",
    ) else {
        return;
    };
    let ids = [owner(1), owner(2), Uuid::max()];
    let times = [at(-1), at(0), at(1)];
    let mut rows = Vec::new();
    let mut n = 0;
    for owner in ids.iter().map(|u| Value::from(*u)).chain([Value::Null]) {
        for at in times.iter().map(|t| Value::from(*t)).chain([Value::Null]) {
            n += 1;
            db.execute(
                "INSERT INTO events (id, owner, at) VALUES ($1, $2, $3)",
                &[Value::Int(n), owner.clone(), at.clone()],
            );
            rows.push(
                MapRow::new()
                    .with(C::Id, n)
                    .with(C::Owner, owner.clone())
                    .with(C::At, at),
            );
        }
    }

    let preds: Vec<Pred<C>> = vec![
        Pred::eq(C::Owner, owner(2)),
        Pred::ne(C::Owner, owner(2)),
        Pred::lt(C::Owner, owner(2)),
        Pred::ge(C::Owner, Uuid::max()),
        Pred::is(C::Owner, owner(1)),
        Pred::is_not(C::Owner, owner(1)),
        Pred::is_null(C::Owner),
        Pred::one_of(C::Owner, [owner(1), Uuid::max()]),
        Pred::none_of(C::Owner, [owner(1)]),
        Pred::eq(C::At, at(0)),
        Pred::lt(C::At, at(0)),
        Pred::ge(C::At, at(0)),
        Pred::is_not(C::At, at(1)),
        Pred::one_of(C::At, [at(-1), at(1)]),
        Pred::negate(Pred::any([
            Pred::eq(C::Owner, owner(1)),
            Pred::gt(C::At, at(0)),
        ])),
    ];
    for pred in &preds {
        if let Err(e) = check::<Event, _, _>(pred, &rows, PG, &mut db) {
            panic!("{pred:?}: {e}");
        }
    }
}
