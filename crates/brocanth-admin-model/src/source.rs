//! The macro, and the per-entity work it expands to.
//!
//! Every function here is generic over one entity, so the macro's arms are
//! one line each and the logic is written once rather than per model.

use brocanth_admin::data::{DataError, Locked, Scope, Search};
use brocanth_admin::model::Row;
use brocanth_admin::query::{Direction, Sort};
use brocanth_admin::row_policy::Dialect;
use brocanth_model::sea_orm::sea_query::{Alias, Expr, ExprTrait, Func, IntoCondition, LikeExpr};
use brocanth_model::sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DbBackend, EntityTrait, Iterable, Order,
    PaginatorTrait, PrimaryKeyToColumn, QueryFilter, QueryOrder, QuerySelect, Select,
    TransactionSession, TransactionTrait,
};
use std::str::FromStr;

/// `WHERE` for the admin's row policy.
///
/// The condition is `brocanth-policy`'s SQL text and bind list, turned into
/// a SeaORM condition by `brocanth_model::policy::condition`: the adapter
/// any `model` application uses for `Entity::find()`.
///
/// A condition that does not match the connection's backend is refused
/// rather than run. `Dialect` has one variant, `Postgres`, but `DbBackend`
/// has others, so a connection to MySQL still gets a clear error and not a
/// syntax error.
pub fn scope_of(scope: Scope<'_>, backend: DbBackend) -> Result<Condition, DataError> {
    let Some(condition) = scope.condition() else {
        return Ok(Condition::all());
    };
    let matches = matches!(
        (condition.dialect(), backend),
        (Dialect::Postgres, DbBackend::Postgres)
    );
    if !matches {
        return Err(DataError::backend(format!(
            "the row policy was compiled for {:?}, but this database is {backend:?}\n\
             help: set `AdminResource::DIALECT` to the dialect of this database",
            condition.dialect()
        )));
    }
    Ok(brocanth_model::policy::condition(condition))
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
/// Not a bare `LIKE`: it is case-sensitive on Postgres, so searching "ada"
/// would not find "Ada Lovelace". `LOWER` on both sides makes the match
/// case-insensitive.
///
/// The cast is because a searchable column need not be text. `LOWER` of an
/// integer is an error on Postgres, and the admin lets you search an id.
///
/// The term is matched as written: a `%` or an `_` in it is escaped, so it
/// matches itself and not any text.
pub fn search_of<E: EntityTrait>(search: Search<'_>) -> Result<Condition, DataError> {
    if search.is_empty() {
        return Ok(Condition::all());
    }
    let pattern = format!("%{}%", like_escaped(&search.term.to_lowercase()));
    let mut any = Condition::any();
    for name in search.columns {
        let column = columns_of::<E>(name)?;
        let text = Func::cast_as(Expr::col(column), Alias::new("TEXT"));
        any = any.add(
            Expr::expr(Func::lower(text))
                .like(LikeExpr::new(pattern.clone()).escape('\\'))
                .into_condition(),
        );
    }
    Ok(any)
}

/// `term` with `\`, `%` and `_` escaped by `\`, for a `LIKE … ESCAPE '\'`.
pub fn like_escaped(term: &str) -> String {
    let mut out = String::with_capacity(term.len());
    for c in term.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `WHERE column = ?` for the admin's exact-match filter: the children of
/// one record, for an inline.
///
/// The column went through `FromStr` and the value is a bind, as in
/// [`single_key`].
pub fn equals_of<E: EntityTrait>(
    filter: Option<brocanth_admin::data::Equals<'_>>,
) -> Result<Condition, DataError> {
    let Some(filter) = filter else {
        return Ok(Condition::all());
    };
    let column = columns_of::<E>(filter.column)?;
    Ok(Condition::all().add(column.eq(value_of(filter.value))))
}

/// One admin value as a SeaORM bind, with its own type: an integer binds as
/// an integer, so PostgreSQL compares it with an integer column.
pub fn value_of(value: &brocanth_admin::model::Value) -> brocanth_model::sea_orm::Value {
    use brocanth_admin::model::Value;
    match value {
        Value::Null => Option::<String>::None.into(),
        Value::Text(s) => s.clone().into(),
        Value::Int(n) => (*n).into(),
        Value::Float(f) => (*f).into(),
        Value::Bool(b) => (*b).into(),
    }
}

/// `ORDER BY`, applied to a select: the sort asked for, then the primary
/// key.
///
/// The key is always the last term, also with no sort, so `LIMIT` and
/// `OFFSET` cut one stable order. Without it Postgres may return the rows of
/// two pages in any order, and a row shows on two pages or on none.
pub fn order_of<E: EntityTrait>(
    select: Select<E>,
    sort: Option<&Sort>,
) -> Result<Select<E>, DataError> {
    let mut select = select;
    if let Some(sort) = sort {
        let column = columns_of::<E>(&sort.column)?;
        let order = match sort.direction {
            Direction::Ascending => Order::Asc,
            Direction::Descending => Order::Desc,
        };
        select = select.order_by(column, order);
    }
    for key in E::PrimaryKey::iter() {
        select = select.order_by(key.into_column(), Order::Asc);
    }
    Ok(select)
}

/// One page, as the admin's rows.
pub async fn rows_of<E: EntityTrait>(
    db: &impl ConnectionTrait,
    request: &brocanth_admin::data::ListRequest<'_>,
) -> Result<Vec<Row>, DataError> {
    let select = E::find()
        .filter(search_of::<E>(request.search)?)
        .filter(equals_of::<E>(request.filter)?)
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
    db: &impl ConnectionTrait,
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

/// The rows whose `column` holds one of `keys`, guarded by the scope:
/// `WHERE column IN (…) AND <condition>`. One query for a batch of foreign
/// key labels. A key the column type cannot hold matches nothing.
pub async fn rows_by<E: EntityTrait>(
    db: &impl ConnectionTrait,
    column: &str,
    keys: &[String],
    scope: Scope<'_>,
) -> Result<Vec<Row>, DataError> {
    let column = columns_of::<E>(column)?;
    let values: Vec<Expr> = keys
        .iter()
        .filter_map(|text| key_value(&column, text))
        .collect();
    if values.is_empty() {
        return Ok(Vec::new());
    }
    let found = E::find()
        .filter(Expr::col(column.as_column_ref()).is_in(values))
        .filter(scope_of(scope, db.get_database_backend())?)
        .into_json()
        .all(db)
        .await
        .map_err(|e| DataError::backend(e.to_string()))?;
    Ok(found.iter().map(crate::row_from_json).collect())
}

/// How many rows match the search and the scope. The scope is the one the
/// page gets, so the total and the pages describe one set.
pub async fn count_of<E: EntityTrait>(
    db: &impl ConnectionTrait,
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
///
/// The key arrives as text, from a URL, and binds as its column's type:
/// PostgreSQL has no `integer = text` or `uuid = text` operator. A key the
/// column cannot hold, such as `abc` for an integer, matches no row, so the
/// admin answers 404 and not a database error.
pub fn single_key<E: EntityTrait>(
    key: &brocanth_admin::model::PrimaryKey,
    table: &str,
) -> Result<Condition, DataError> {
    let (name, text) = crate::one_key(key, table)?;
    let column = columns_of::<E>(&name)?;
    let condition = match key_value(&column, &text) {
        Some(value) => Expr::col(column.as_column_ref()).eq(value),
        None => Expr::val(false),
    };
    Ok(Condition::all().add(condition))
}

/// One key, as text, bound as `column`'s type, or `None` when the column
/// cannot hold it.
///
/// An integer binds as an integer. A UUID binds as text cast to `uuid`, so
/// the crate needs no UUID library; its shape is checked first, so a bad
/// one is no row rather than a cast error.
fn key_value<C: ColumnTrait>(column: &C, text: &str) -> Option<Expr> {
    use brocanth_model::sea_orm::sea_query::ColumnType;
    match column.def().get_column_type() {
        ColumnType::TinyInteger
        | ColumnType::SmallInteger
        | ColumnType::Integer
        | ColumnType::BigInteger
        | ColumnType::TinyUnsigned
        | ColumnType::SmallUnsigned
        | ColumnType::Unsigned
        | ColumnType::BigUnsigned => text.parse::<i64>().ok().map(Expr::val),
        ColumnType::Uuid => {
            is_uuid(text).then(|| Expr::val(text.to_owned()).cast_as(Alias::new("uuid")))
        }
        _ => Some(Expr::val(text.to_owned())),
    }
}

/// Whether `text` is a UUID as PostgreSQL reads one: 32 hex digits, with or
/// without hyphens, with or without braces.
fn is_uuid(text: &str) -> bool {
    let inner = text
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .unwrap_or(text);
    let digits: Vec<char> = inner.chars().filter(|c| *c != '-').collect();
    digits.len() == 32 && digits.iter().all(char::is_ascii_hexdigit)
}

/// The JSON `set_from_json` takes.
pub fn json_of(
    model: &brocanth_admin::model::ModelDescriptor,
    values: &[(String, brocanth_admin::model::Value)],
) -> serde_json::Value {
    crate::json_from_values(model, values)
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
pub async fn insert_of<E, C>(
    db: &C,
    model: &brocanth_admin::model::ModelDescriptor,
    values: &[(String, brocanth_admin::model::Value)],
) -> Result<brocanth_admin::model::PrimaryKey, DataError>
where
    E: EntityTrait,
    C: ConnectionTrait + TransactionTrait,
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
        .set_from_json(crate::json_from_values(model, values))
        .map_err(unreadable)?;
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
/// transaction can make it stop matching in between.
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
pub async fn update_of<E, C>(
    db: &C,
    model: &brocanth_admin::model::ModelDescriptor,
    key: &brocanth_admin::model::PrimaryKey,
    values: &[(String, brocanth_admin::model::Value)],
    scope: Scope<'_>,
) -> Result<u64, DataError>
where
    E: EntityTrait,
    C: ConnectionTrait + TransactionTrait,
    E::Model: Send + Sync + IntoActiveModel<E::ActiveModel> + serde::Serialize,
    for<'de> E::Model: serde::Deserialize<'de>,
    E::ActiveModel: ActiveModelTrait<Entity = E>
        + Send
        + Sync
        + brocanth_model::sea_orm::TryIntoModel<E::Model>,
{
    Ok(update_locked_of::<E, C>(db, model, key, values, scope)
        .await?
        .rows)
}

/// [`update_of`], and the row as it read it under the lock: what the
/// admin journals as the values before the write.
pub async fn update_locked_of<E, C>(
    db: &C,
    model: &brocanth_admin::model::ModelDescriptor,
    key: &brocanth_admin::model::PrimaryKey,
    values: &[(String, brocanth_admin::model::Value)],
    scope: Scope<'_>,
) -> Result<Locked, DataError>
where
    E: EntityTrait,
    C: ConnectionTrait + TransactionTrait,
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
        return Ok(Locked::new(0, None));
    };
    // The row as it stands, with the submitted fields written over it.
    let mut merged = serde_json::to_value(&found).map_err(|e| DataError::backend(e.to_string()))?;
    let before = crate::row_from_json(&merged);
    let submitted = crate::json_from_values(model, values);
    if let (Some(into), Some(from)) = (merged.as_object_mut(), submitted.as_object()) {
        for (name, value) in from {
            into.insert(name.clone(), value.clone());
        }
    }

    // Parse the merged row, then mark only the columns whose value moves.
    // The `UPDATE` then names those columns alone: a role with column
    // grants (an unguarded write mask, docs/rls.md) may update the fields
    // it was given without the statement touching the others.
    let mut parsed = <E::ActiveModel as ActiveModelTrait>::default();
    parsed.set_from_json(merged).map_err(unreadable)?;
    let mut active = found.into_active_model();
    for column in E::Column::iter() {
        let Some(next) = parsed.get(column).into_value() else {
            continue;
        };
        if active.get(column).into_value().as_ref() != Some(&next) {
            active.set(column, next);
        }
    }
    if active.is_changed() {
        active.update(&txn).await.map_err(invalid)?;
    }
    txn.commit().await.map_err(backend)?;
    Ok(Locked::new(1, Some(before)))
}

/// Removes a row, guarded by the scope, and reports the rows it removed.
///
/// Through the loaded model and not `delete_many`, for the reason above: a
/// `before_delete` hook is a hook. Guarded the way [`update_of`] is, in one
/// transaction with the row locked.
pub async fn delete_of<E, C>(
    db: &C,
    model: &brocanth_admin::model::ModelDescriptor,
    key: &brocanth_admin::model::PrimaryKey,
    scope: Scope<'_>,
) -> Result<u64, DataError>
where
    E: EntityTrait,
    C: ConnectionTrait + TransactionTrait,
    E::Model:
        Send + Sync + ModelTrait<Entity = E> + IntoActiveModel<E::ActiveModel> + serde::Serialize,
    E::ActiveModel:
        ActiveModelTrait<Entity = E> + brocanth_model::sea_orm::ActiveModelBehavior + Send,
{
    Ok(delete_locked_of::<E, C>(db, model, key, scope).await?.rows)
}

/// [`delete_of`], and the row as it read it under the lock.
pub async fn delete_locked_of<E, C>(
    db: &C,
    model: &brocanth_admin::model::ModelDescriptor,
    key: &brocanth_admin::model::PrimaryKey,
    scope: Scope<'_>,
) -> Result<Locked, DataError>
where
    E: EntityTrait,
    C: ConnectionTrait + TransactionTrait,
    E::Model:
        Send + Sync + ModelTrait<Entity = E> + IntoActiveModel<E::ActiveModel> + serde::Serialize,
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
        return Ok(Locked::new(0, None));
    };
    let before = serde_json::to_value(&found)
        .map(|json| crate::row_from_json(&json))
        .map_err(|e| DataError::backend(e.to_string()))?;
    let removed = found.delete(&txn).await.map_err(backend)?;
    txn.commit().await.map_err(backend)?;
    Ok(Locked::new(removed.rows_affected, Some(before)))
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
            "`{}` was written but its key could not be read back\n\
             help: check that the entity's primary key columns serialise under \
             their column names",
            model.key
        ))
    })
}

/// A database error on a write, as the admin spells it.
///
/// A validation hook of `brocanth-model` fails with a `DbErr` that carries
/// its per-field errors ([`brocanth_model::Errors::from_db_err`]). Those
/// become [`DataError::Invalid`], so each message lands beside its own
/// input. Every other error is a backend error.
fn invalid(error: brocanth_model::sea_orm::DbErr) -> DataError {
    crate::db_error(error)
}

/// The submitted values did not fit the entity: a text that is not a
/// number for an integer column, for example. The person can fix it, so it
/// is a form error and not a backend error.
fn unreadable(error: brocanth_model::sea_orm::DbErr) -> DataError {
    if let Some(errors) = brocanth_model::Errors::from_db_err(&error) {
        return DataError::Invalid(crate::field_errors(&errors));
    }
    let mut errors = brocanth_admin::data::FieldErrors::new();
    errors.add_base(format!("The values could not be saved: {error}"));
    DataError::Invalid(errors)
}
