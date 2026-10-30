//! The macro, and the per-entity work it expands to.
//!
//! Every function here is generic over one entity, so the macro's arms are
//! one line each and the logic is written once rather than per model.

use brocanth_admin::data::{DataError, Scope, Search};
use brocanth_admin::model::Row;
use brocanth_admin::query::{Direction, Sort};
use brocanth_admin::row_policy::{Dialect, Value as PolicyValue};
use brocanth_model::sea_orm::sea_query::{Alias, Expr, ExprTrait, Func, IntoCondition};
use brocanth_model::sea_orm::{
    ColumnTrait, Condition, DbBackend, EntityTrait, Order, PaginatorTrait, QueryFilter, QueryOrder,
    QuerySelect, Select, TransactionTrait,
};
use std::str::FromStr;

/// `WHERE` for the admin's row policy.
///
/// The condition is `brocanth-policy`'s SQL text and bind list, spliced in
/// with `Expr::cust_with_values`. sea-query reads the placeholders the
/// dialect uses — `?` for SQLite, `$n` for PostgreSQL — and numbers them
/// into the statement it builds, so the condition's own numbering survives
/// whatever binds come before it. Every value stays a bind: nothing from the
/// condition's values reaches the text.
///
/// A condition compiled for another dialect is refused rather than run,
/// because `IS NOT DISTINCT FROM` on SQLite and `?` on PostgreSQL are both
/// syntax errors at best.
pub fn scope_of(scope: Scope<'_>, backend: DbBackend) -> Result<Condition, DataError> {
    let Some(condition) = scope.condition() else {
        return Ok(Condition::all());
    };
    let matches = matches!(
        (condition.dialect(), backend),
        (Dialect::Postgres, DbBackend::Postgres) | (Dialect::Sqlite, DbBackend::Sqlite)
    );
    if !matches {
        return Err(DataError::backend(format!(
            "the row policy was compiled for {:?}, but this database is {backend:?}; set \
             `AdminResource::DIALECT` to match",
            condition.dialect()
        )));
    }
    let binds: Vec<brocanth_model::sea_orm::Value> =
        condition.binds().iter().map(bind_of).collect();
    Ok(Condition::all().add(Expr::cust_with_values(condition.sql().to_owned(), binds)))
}

/// One policy value as a SeaORM bind.
///
/// A null binds as a text null. It is rare here: `brocanth-policy` compiles
/// an absent subject value to `FALSE` or `TRUE` through `Bound::or_deny` and
/// `or_allow`, not to a `NULL` bind.
fn bind_of(value: &PolicyValue) -> brocanth_model::sea_orm::Value {
    match value {
        PolicyValue::Null => Option::<String>::None.into(),
        PolicyValue::Bool(b) => (*b).into(),
        PolicyValue::Int(n) => (*n).into(),
        PolicyValue::Float(f) => (*f).into(),
        PolicyValue::Text(s) => s.clone().into(),
        PolicyValue::Bytes(b) => b.clone().into(),
    }
}

/// A column of `E` by name, or a refusal.
///
/// `FromStr` and not string interpolation, which is also the only reason a
/// sort parameter out of a query string cannot reach SQL: a name that is
/// not a column never becomes one.
pub fn columns_of<E: EntityTrait>(name: &str) -> Result<E::Column, DataError> {
    E::Column::from_str(name).map_err(|_| crate::unknown_column(name))
}

/// `WHERE` for a search, over the columns the admin said are searchable.
///
/// `LOWER(CAST(col AS TEXT)) LIKE '%term%'`, and every part of that is
/// load-bearing.
///
/// Not `ILIKE`: SeaORM's own documentation marks it "Postgres Only", and it
/// is a syntax error on SQLite, which the same admin runs over.
///
/// Not a bare `LIKE` either: it is case-sensitive on Postgres, so searching
/// "ada" would not find "Ada Lovelace" in production while passing every
/// test on SQLite, where `LIKE` is case-insensitive for ASCII. That is the
/// worst shape a bug can have.
///
/// The cast is because a searchable column need not be text. `LOWER` of an
/// integer is an error on Postgres, and the admin lets you search an id.
pub fn search_of<E: EntityTrait>(search: Search<'_>) -> Result<Condition, DataError> {
    if search.is_empty() {
        return Ok(Condition::all());
    }
    let pattern = format!("%{}%", search.term.to_lowercase());
    let mut any = Condition::any();
    for name in search.columns {
        let column = columns_of::<E>(name)?;
        let text = Func::cast_as(Expr::col(column), Alias::new("TEXT"));
        any = any.add(
            Expr::expr(Func::lower(text))
                .like(pattern.clone())
                .into_condition(),
        );
    }
    Ok(any)
}

/// `ORDER BY`, applied to a select.
pub fn order_of<E: EntityTrait>(
    select: Select<E>,
    sort: Option<&Sort>,
) -> Result<Select<E>, DataError> {
    let Some(sort) = sort else {
        return Ok(select);
    };
    let column = columns_of::<E>(&sort.column)?;
    let order = match sort.direction {
        Direction::Ascending => Order::Asc,
        Direction::Descending => Order::Desc,
    };
    Ok(select.order_by(column, order))
}

/// One page, as the admin's rows.
pub async fn rows_of<E: EntityTrait>(
    db: &brocanth_model::sea_orm::DatabaseConnection,
    request: &brocanth_admin::data::ListRequest<'_>,
) -> Result<Vec<Row>, DataError> {
    let select = E::find()
        .filter(search_of::<E>(request.search)?)
        .filter(scope_of(request.scope, db.get_database_backend())?);
    let select = order_of::<E>(select, request.sort)?;
    let found = select
        .limit(request.limit as u64)
        .offset(request.offset as u64)
        .into_json()
        .all(db)
        .await
        .map_err(|e| DataError::backend(e.to_string()))?;
    Ok(found.iter().map(crate::row_from_json).collect())
}

/// One row by its single-column key, guarded by the scope:
/// `WHERE pk = ? AND <condition>`.
pub async fn row_of<E: EntityTrait>(
    db: &brocanth_model::sea_orm::DatabaseConnection,
    key: &brocanth_admin::model::PrimaryKey,
    table: &str,
    scope: Scope<'_>,
) -> Result<Option<Row>, DataError> {
    let found = E::find()
        .filter(single_key::<E>(key, table)?)
        .filter(scope_of(scope, db.get_database_backend())?)
        .into_json()
        .one(db)
        .await
        .map_err(|e| DataError::backend(e.to_string()))?;
    Ok(found.as_ref().map(crate::row_from_json))
}

/// How many rows match the search and the scope. The scope is the one the
/// page gets, so the total and the pages describe one set.
pub async fn count_of<E: EntityTrait>(
    db: &brocanth_model::sea_orm::DatabaseConnection,
    search: Search<'_>,
    scope: Scope<'_>,
) -> Result<usize, DataError>
where
    E::Model: Send + Sync,
{
    E::find()
        .filter(search_of::<E>(search)?)
        .filter(scope_of(scope, db.get_database_backend())?)
        .count(db)
        .await
        .map(|n| n as usize)
        .map_err(|e| DataError::backend(e.to_string()))
}

/// A `WHERE` matching one row by its key.
///
/// The value is bound and never interpolated, and the column went through
/// `FromStr`, so neither half of this reaches SQL as text.
pub fn single_key<E: EntityTrait>(
    key: &brocanth_admin::model::PrimaryKey,
    table: &str,
) -> Result<Condition, DataError> {
    let (name, value) = crate::one_key(key, table)?;
    let column = columns_of::<E>(&name)?;
    Ok(Condition::all().add(column.eq(value)))
}

/// The JSON `set_from_json` takes.
pub fn json_of(values: &[(String, brocanth_admin::model::Value)]) -> serde_json::Value {
    crate::json_from_values(values)
}

/// One entity's registry entry, columns and all.
///
/// The columns are the point. A `RegistryEntry` with none of them is a
/// model the admin lists with an empty table and a form with no fields,
/// which renders without error and shows nothing — the failure this
/// function exists to prevent, and the one the first version of the macro
/// shipped.
///
/// `describe` is `brocanth-model`'s, so the column spellings are the ones
/// the admin's `ColumnKind::from_type_name` was written against.
pub fn entry_of<E: EntityTrait + Default>(
    module_path: &str,
    backend: brocanth_model::sea_orm::DbBackend,
) -> brocanth_admin::discovery::RegistryEntry {
    use brocanth_admin::adapter::{entry_from_schema, SchemaColumn};

    let info = brocanth_model::registry::describe(E::default(), backend);
    let columns: Vec<SchemaColumn<'_>> = info
        .columns
        .iter()
        .map(|c| SchemaColumn {
            name: &c.name,
            type_name: &c.type_name,
            nullable: c.nullable,
            unique: c.unique,
            primary_key: c.primary_key,
        })
        .collect();
    entry_from_schema(module_path, &info.table, &columns)
}

// ---------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------
//
// Every one of these goes through an `ActiveModel`. PLAN §7 trap 2 is the
// reason and it is not a style preference: `insert_many`, `update_many` and
// raw SQL all bypass SeaORM's hooks, and the hooks are where
// `brocanth-model`'s validations run. A data source that built the SQL
// itself would be shorter, would pass its own tests, and would silently
// skip every validation the application declared.

use brocanth_model::sea_orm::{ActiveModelTrait, IntoActiveModel, ModelTrait};

/// Writes a new row and reports its key.
pub async fn insert_of<E>(
    db: &brocanth_model::sea_orm::DatabaseConnection,
    model: &brocanth_admin::model::ModelDescriptor,
    values: &[(String, brocanth_admin::model::Value)],
) -> Result<brocanth_admin::model::PrimaryKey, DataError>
where
    E: EntityTrait,
    // The bounds `set_from_json` asks for, written out rather than hidden
    // behind a helper trait: they are what tells you the entity has to
    // round-trip through JSON, which is the whole mechanism here.
    E::Model: Send + Sync + IntoActiveModel<E::ActiveModel> + serde::Serialize,
    for<'de> E::Model: serde::Deserialize<'de>,
    E::ActiveModel: ActiveModelTrait<Entity = E>
        + Send
        + Sync
        + brocanth_model::sea_orm::TryIntoModel<E::Model>,
{
    // `ActiveModelTrait::default`, not `Default::default`: both are in
    // scope and only one of them makes every column `NotSet`.
    let mut active = <E::ActiveModel as ActiveModelTrait>::default();
    active
        .set_from_json(crate::json_from_values(values))
        .map_err(invalid)?;
    let written = active.insert(db).await.map_err(invalid)?;
    key_of::<E>(model, &written)
}

/// Writes over an existing row, guarded by the scope, and reports the rows
/// it changed: one, or zero when the row is gone or no longer matches.
///
/// # The guard, and why it is a lock and not a `WHERE`
///
/// PLAN.md §9 obligation 2 wants `UPDATE … WHERE id = ? AND <condition>`.
/// `ActiveModel::update` writes `WHERE id = ?` and offers no way to add to
/// it, and `update_many` would skip the hooks. So the guard is a
/// transaction: the row is read with `WHERE id = ? AND <condition>` and
/// `FOR UPDATE`, and the `ActiveModel` writes it before the transaction
/// ends. On PostgreSQL the lock holds the row until the commit, so no other
/// transaction can make it stop matching in between. SQLite has no row
/// lock (sea-query drops the clause) and serialises writers instead; a
/// writer that commits between the read and the write fails this
/// transaction, it does not slip past it.
///
/// Loaded first, so the `ActiveModel` carries its primary key and SeaORM
/// issues an `UPDATE` rather than an `INSERT`.
///
/// The submitted values are merged onto the row as it stands, and that
/// merge is not optional. `set_from_json` says it "will not alter the
/// primary key values", which reads as though it leaves other absent
/// columns alone; it does not. It is `*self = Self::from_json(json)` with
/// the key put back, so every column missing from the object is cleared.
/// Handing it only the submitted fields emptied the rest of the record --
/// a test caught it, having been written in the belief that the opposite
/// was true.
pub async fn update_of<E>(
    db: &brocanth_model::sea_orm::DatabaseConnection,
    model: &brocanth_admin::model::ModelDescriptor,
    key: &brocanth_admin::model::PrimaryKey,
    values: &[(String, brocanth_admin::model::Value)],
    scope: Scope<'_>,
) -> Result<u64, DataError>
where
    E: EntityTrait,
    E::Model: Send + Sync + IntoActiveModel<E::ActiveModel> + serde::Serialize,
    for<'de> E::Model: serde::Deserialize<'de>,
    E::ActiveModel: ActiveModelTrait<Entity = E>
        + Send
        + Sync
        + brocanth_model::sea_orm::TryIntoModel<E::Model>,
{
    let guard = scope_of(scope, db.get_database_backend())?;
    let txn = db.begin().await.map_err(backend)?;
    let found = E::find()
        .filter(single_key::<E>(key, &model.table)?)
        .filter(guard)
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(backend)?;
    let Some(found) = found else {
        // Gone, or no longer matching. Zero rows, which the admin answers
        // as a lost race; the transaction rolls back when it drops.
        return Ok(0);
    };
    // The row as it stands, with the submitted fields written over it.
    let mut merged = serde_json::to_value(&found).map_err(|e| DataError::backend(e.to_string()))?;
    let submitted = crate::json_from_values(values);
    if let (Some(into), Some(from)) = (merged.as_object_mut(), submitted.as_object()) {
        for (name, value) in from {
            into.insert(name.clone(), value.clone());
        }
    }

    let mut active = found.into_active_model();
    active.set_from_json(merged).map_err(invalid)?;
    active.update(&txn).await.map_err(invalid)?;
    txn.commit().await.map_err(backend)?;
    Ok(1)
}

/// Removes a row, guarded by the scope, and reports the rows it removed.
///
/// Through the loaded model and not `delete_many`, for the reason above: a
/// `before_delete` hook is a hook. Guarded the way [`update_of`] is, in one
/// transaction with the row locked.
pub async fn delete_of<E>(
    db: &brocanth_model::sea_orm::DatabaseConnection,
    model: &brocanth_admin::model::ModelDescriptor,
    key: &brocanth_admin::model::PrimaryKey,
    scope: Scope<'_>,
) -> Result<u64, DataError>
where
    E: EntityTrait,
    E::Model: Send + Sync + ModelTrait<Entity = E> + IntoActiveModel<E::ActiveModel>,
    E::ActiveModel:
        ActiveModelTrait<Entity = E> + brocanth_model::sea_orm::ActiveModelBehavior + Send,
{
    let guard = scope_of(scope, db.get_database_backend())?;
    let txn = db.begin().await.map_err(backend)?;
    let found = E::find()
        .filter(single_key::<E>(key, &model.table)?)
        .filter(guard)
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(backend)?;
    let Some(found) = found else {
        // Gone, or no longer matching: zero rows, never success.
        return Ok(0);
    };
    let removed = found.delete(&txn).await.map_err(backend)?;
    txn.commit().await.map_err(backend)?;
    Ok(removed.rows_affected)
}

fn backend(error: brocanth_model::sea_orm::DbErr) -> DataError {
    DataError::backend(error.to_string())
}

/// The key of a model that was just written.
fn key_of<E: EntityTrait>(
    model: &brocanth_admin::model::ModelDescriptor,
    written: &E::Model,
) -> Result<brocanth_admin::model::PrimaryKey, DataError>
where
    E::Model: Send + Sync + serde::Serialize,
{
    let json = serde_json::to_value(written).map_err(|e| DataError::backend(e.to_string()))?;
    let row = crate::row_from_json(&json);
    brocanth_admin::model::PrimaryKey::from_row(model, &row).ok_or_else(|| {
        DataError::backend(format!(
            "`{}` was written but its key could not be read back",
            model.key
        ))
    })
}

/// A database error on a write, as the admin spells it.
///
/// Everything is a backend error here on purpose, and it is a known gap:
/// `brocanth_model::Error::errors()` carries per-field entries, and copying
/// them into `FieldErrors` is what puts a message beside its own input
/// rather than in a banner at the top. That needs the model layer's error
/// type to survive `DbErr`, which it does not yet.
fn invalid(error: brocanth_model::sea_orm::DbErr) -> DataError {
    DataError::backend(error.to_string())
}
