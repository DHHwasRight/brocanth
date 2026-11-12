//! A `brocanth-policy` condition as a SeaORM `WHERE`.
//!
//! Behind the `policy` feature. `brocanth-policy` emits SQL text and a bind
//! list and knows no ORM; this is the adapter between the two, so a list
//! page adds the policy's `condition()` to `Entity::find()` and keeps its
//! pagination: the rows a subject may not read never load.
//!
//! ```ignore
//! let rows = post::Entity::find()
//!     .filter(brocanth_model::policy::condition(verified.condition()))
//!     .paginate(db, 20);
//! ```
//!
//! The text is spliced in with `Expr::cust_with_values`. sea-query reads the
//! `$n` placeholders and numbers them into the statement it builds, so the
//! condition's own numbering survives whatever binds come before it. Every
//! value stays a bind: nothing from the values reaches the text.

use brocanth_policy::{Condition, Dialect, Value};
use sea_orm::sea_query::Expr;

/// The policy's condition, as a SeaORM condition.
///
/// Brocanth is Postgres only, and so are `brocanth-policy`'s dialect and
/// this crate's connection: the match below has one arm, and a new dialect
/// stops this from compiling until it is handled.
pub fn condition(condition: &Condition) -> sea_orm::Condition {
    match condition.dialect() {
        Dialect::Postgres => {}
    }
    let binds: Vec<sea_orm::Value> = condition.binds().iter().map(bind).collect();
    sea_orm::Condition::all().add(Expr::cust_with_values(condition.sql().to_owned(), binds))
}

/// One policy value as a SeaORM bind.
///
/// A null binds as a text null. It is rare: `brocanth-policy` compiles an
/// absent subject value to `FALSE` or `TRUE` through `Bound::or_deny` and
/// `or_allow`, not to a `NULL` bind.
pub fn bind(value: &Value) -> sea_orm::Value {
    match value {
        Value::Null => Option::<String>::None.into(),
        Value::Bool(b) => (*b).into(),
        Value::Int(n) => (*n).into(),
        Value::Float(f) => (*f).into(),
        Value::Text(s) => s.clone().into(),
        Value::Bytes(b) => b.clone().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_value_binds_with_its_own_type() {
        assert_eq!(bind(&Value::Int(7)), sea_orm::Value::BigInt(Some(7)));
        assert_eq!(bind(&Value::Bool(true)), sea_orm::Value::Bool(Some(true)));
        assert_eq!(
            bind(&Value::Text("a".into())),
            sea_orm::Value::String(Some("a".into()))
        );
        assert_eq!(bind(&Value::Null), sea_orm::Value::String(None));
    }
}
