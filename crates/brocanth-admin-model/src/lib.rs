#![doc = include_str!("../README.md")]
//!
//! # Design notes
//!
//! The admin's data source over SeaORM.
//!
//! [`AdminData`](brocanth_admin::data::AdminData) is dynamic: a table name and a column name arrive as
//! runtime strings, because the admin is one screen serving every model.
//! SeaORM is static: an entity is a type, a column is a variant, and there
//! is no reflection to turn `"post"` into `post::Entity`. Django gets this
//! free from Python; here something has to bridge it.
//!
//! Three SeaORM facilities do, and the whole design is downstream of them:
//!
//! 1. **`ColumnTrait: FromStr`**, so `"title"` becomes `Column::Title` and a
//!    name that is not a column is an error rather than injected SQL.
//! 2. **`Select::into_json`**, so a read comes back as `serde_json::Value`
//!    and one function shapes every model's rows.
//! 3. **`ActiveModelTrait::set_from_json`**, so a write can be assembled
//!    from name/value pairs and still go through an `ActiveModel`.
//!
//! That third one is the one that matters. PLAN §7 trap 2 says writes go
//! through `ActiveModel::insert` / `::update` / `::delete` because that is
//! where the hooks run, and the hooks are where `brocanth-model`'s
//! validations live. A data source that built SQL by hand would be faster
//! to write, would pass its tests, and would silently skip every validation
//! in the application. So every write here loads or builds an `ActiveModel`
//! and goes through SeaORM.
//!
//! # One line per entity
//!
//! SeaORM's registry carries a module path and nothing else, so "walk every
//! model's columns" cannot be a loop — the entity type has to be named. The
//! admin's own `adapter` module says so and calls it the single thing that
//! is not automatic. It is one line:
//!
//! ```ignore
//! use brocanth_admin_model::admin_source;
//!
//! admin_source! {
//!     pub struct Models;
//!     "blog::models::user" => blog::models::user::Entity,
//!     "blog::models::post" => blog::models::post::Entity,
//! }
//!
//! let data = std::sync::Arc::new(Models::new(db));
//! ```
//!
//! and `Models` implements [`AdminData`](brocanth_admin::data::AdminData). Adding an entity is a line; a
//! model that is not listed is not in the admin, which is the right default
//! for a screen that can delete things.
//!
//! # What it does not do
//!
//! **No joins.** A foreign key is declared with `ModelAdmin::foreign_key`
//! and resolved by the admin with a second read, because SeaORM's relations
//! are an enum on the entity and not in the column schema. See the admin's
//! `adapter` module, which measured that.
//!
//! **No composite keys.** A row is addressed by one key column, for a read
//! and for a write alike. A composite key is refused by name, with a
//! `help:` line, rather than half-supported.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use brocanth_admin::data::{DataError, FieldErrors};
use brocanth_admin::model::{ColumnKind, ModelDescriptor, PrimaryKey, Row, Value};

pub use brocanth_admin;
pub use brocanth_model;
pub use serde_json;

mod source;

pub use async_trait::async_trait;
pub use brocanth_db::DbIdentity;

/// How a request's principal becomes the database identity its statements
/// run as. `admin_source!`'s `as_identity` takes one.
pub type Identify =
    std::sync::Arc<dyn for<'p> Fn(brocanth_admin::Principal<'p>) -> DbIdentity + Send + Sync>;

/// A transaction on `db` that runs as `me`: `BEGIN`, then `SET LOCAL ROLE`
/// and the settings. For `admin_source!`.
pub async fn begin_as(
    db: &brocanth_model::sea_orm::DatabaseConnection,
    me: &DbIdentity,
) -> Result<brocanth_model::sea_orm::DatabaseTransaction, DataError> {
    use brocanth_model::sea_orm::TransactionTrait as _;
    let txn = db.begin().await.map_err(db_error)?;
    brocanth_model::orm::assume(&txn, me)
        .await
        .map_err(db_error)?;
    Ok(txn)
}

/// Ends a transaction [`begin_as`] opened: commits it when `out` is `Ok`,
/// rolls it back when not. For `admin_source!`.
pub async fn end<T>(
    txn: brocanth_model::sea_orm::DatabaseTransaction,
    out: Result<T, DataError>,
) -> Result<T, DataError> {
    match out {
        Ok(value) => {
            txn.commit().await.map_err(db_error)?;
            Ok(value)
        }
        Err(error) => {
            // The error says why; a failed rollback closes the connection,
            // and its transaction does not commit.
            let _ = txn.rollback().await;
            Err(error)
        }
    }
}

/// The identity `who` builds for an anonymous principal, checked as
/// `Tx::assume` would check it: a role, a key or a value it refuses fails
/// here, when the admin is built. For `admin_source!`'s `AdminData::check`.
///
/// # Errors
///
/// [`DataError::Backend`] with the refusal and its `help:` line.
pub fn check_identity(who: &Identify) -> Result<(), DataError> {
    who(brocanth_admin::Principal::anonymous())
        .statements()
        .map(drop)
        .map_err(|e| DataError::backend(format!("the admin's database identity is refused: {e}")))
}
pub use source::{
    columns_of, count_of, delete_locked_of, delete_of, entry_of, equals_of, insert_of, json_of,
    like_escaped, order_of, row_of, rows_by, rows_of, scope_of, search_of, single_key,
    update_locked_of, update_of, value_of,
};

/// A module path the source was not given an entity for.
pub fn unknown_model(module_path: &str) -> DataError {
    DataError::UnknownModel(module_path.to_string())
}

/// A database error, as the admin spells a backend failure.
///
/// A validation failure of the model layer is not a database error: it
/// becomes [`DataError::Invalid`], so the form shows each message beside
/// its own field.
pub fn db_error(error: brocanth_model::sea_orm::DbErr) -> DataError {
    match brocanth_model::Errors::from_db_err(&error) {
        Some(errors) => DataError::Invalid(field_errors(&errors)),
        None => DataError::backend(error.to_string()),
    }
}

/// The model layer's validation errors, as the admin's form takes them.
pub fn field_errors(errors: &brocanth_model::Errors) -> FieldErrors {
    let mut out = FieldErrors::new();
    for entry in errors {
        out.add(entry.field.clone(), entry.message.clone());
    }
    out
}

/// What a name that is not a column gets, rather than a query.
pub fn unknown_column(name: &str) -> DataError {
    DataError::backend(format!(
        "`{name}` is not a column on this entity; the admin was asked to \
         sort, search or filter on a name the schema does not have\n\
         help: check the column names given to the `ModelAdmin` against the \
         entity, and list the entity in the `admin_source!` of `models.rs`"
    ))
}

/// Turns a `serde_json` object into the admin's [`Row`].
pub fn row_from_json(json: &serde_json::Value) -> Row {
    let mut row = Row::new();
    let Some(object) = json.as_object() else {
        return row;
    };
    for (name, value) in object {
        row.set(name.clone(), value_from_json(value));
    }
    row
}

/// One JSON value as the admin spells it.
///
/// An object or an array becomes its text, because the admin renders a
/// `jsonb` column in a textarea and round-trips what it was given.
pub fn value_from_json(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => Value::Int(i),
            (None, Some(f)) => Value::Float(f),
            _ => Value::Text(n.to_string()),
        },
        serde_json::Value::String(s) => Value::Text(s.clone()),
        other => Value::Text(other.to_string()),
    }
}

/// The admin's values as the JSON object `set_from_json` takes.
///
/// The column's kind decides. A text value for a JSON column that parses as
/// an object or an array is passed through as JSON, which is how a `jsonb`
/// column survives a round trip through a textarea. Every other text value
/// is a string, so a text column that holds `[1]` or `{}` stays text.
/// SeaORM casts a string to the column's type, or refuses, which is a
/// validation message rather than a panic.
pub fn json_from_values(model: &ModelDescriptor, values: &[(String, Value)]) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    for (name, value) in values {
        let json = model
            .column(name)
            .is_some_and(|c| c.kind == ColumnKind::Json);
        object.insert(name.clone(), json_from_value(value, json));
    }
    serde_json::Value::Object(object)
}

fn json_from_value(value: &Value, json: bool) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Text(s) if json => match serde_json::from_str::<serde_json::Value>(s) {
            // Only a composite: `"7"` parses as a number and must stay the
            // string the form sent.
            Ok(parsed) if parsed.is_object() || parsed.is_array() => parsed,
            _ => serde_json::Value::String(s.clone()),
        },
        Value::Text(s) => serde_json::Value::String(s.clone()),
    }
}

/// The one key column, or a refusal naming the model.
pub fn one_key(key: &PrimaryKey, table: &str) -> Result<(String, String), DataError> {
    let parts = key.parts();
    match parts {
        [(column, value)] => Ok((column.clone(), value.clone())),
        [] => Err(DataError::backend(format!(
            "`{table}` has no primary key, so the admin cannot address a row of it\n\
             help: give the entity a primary key, or hide the model with \
             `Discovery::without`"
        ))),
        _ => Err(DataError::backend(format!(
            "`{table}` has a composite primary key; this data source addresses a \
             row by one key column, so it can neither read nor write a row of it\n\
             help: give the table a single-column key, or hide the model with \
             `Discovery::without`"
        ))),
    }
}

/// A [`AdminData`] over SeaORM, from one line per entity.
///
/// ```ignore
/// admin_source! {
///     /// Every model this admin shows.
///     pub struct Models;
///     "blog::models::user" => blog::models::user::Entity,
///     "blog::models::post" => blog::models::post::Entity,
/// }
///
/// let data = std::sync::Arc::new(Models::new(db));
/// ```
///
/// The module path is the one SeaORM's registry reports, which is what the
/// admin's `Discovery` filters on and what the index screen shows.
///
/// # Why a list and not a loop
///
/// SeaORM's entity registry carries a module path and nothing else. The
/// columns, the primary key and the `ActiveModel` all need the concrete
/// type, and no amount of iteration produces one. So the entity is named,
/// once, and everything else follows from it.
///
/// A model that is not listed is not in the admin. That is the right
/// default for a screen with a delete button on it: appearing there should
/// be a decision somebody made rather than a consequence of compiling.
///
/// The list may be empty. A new project has no entities yet, and an
/// application whose backoffice is only contributed panels -- a queue, a
/// cache, a set of flags -- never needs one. That admin shows "nothing to
/// administer" and its tabs, which is accurate rather than broken.
///
/// # Why the match is written fourteen times
///
/// Once per method, by the macro: nine on the source, five on its
/// transaction. A helper macro holding it once cannot be written: a
/// `macro_rules!` nested inside another cannot capture the outer one's
/// variables, and every arm names a different entity, so no function can
/// hold them either. Fourteen expansions of a two-line match is the cost of the
/// ORM being static and the admin being dynamic.
///
/// # One save, one transaction
///
/// `AdminData::begin` opens a SeaORM `DatabaseTransaction`, and the writes
/// of its `AdminTransaction` run on it through the same functions as the
/// source's own writes. The guarded update and delete open their own
/// transaction for the row lock; on a transaction that is a savepoint, so
/// the row lock lasts until the outer commit.
///
/// # As the staff member's database identity
///
/// On a table under Postgres row-level security, a statement that does not
/// run as an identity sees no row. `as_identity` gives the source a
/// function from the request's principal to a `DbIdentity`
/// (docs/policy-rls-bridge.md, decision 13):
///
/// ```ignore
/// let data = Models::new(db).as_identity(|principal| {
///     DbIdentity::new().role("app_user").set("admin", !principal.is_anonymous())
/// });
/// ```
///
/// Each request then reads through a copy of the source bound to that
/// identity (`AdminData::for_request`). Each read and each write runs in a
/// transaction that assumed it first, and a save with inline rows runs in
/// one. The admin's own row policy still goes into every `WHERE`, so the
/// database checks the same rows twice. `AdminData::check` builds the
/// anonymous principal's identity when the admin is built, so a role or a
/// key the identity refuses fails at start.
///
/// [`AdminData`]: brocanth_admin::data::AdminData
#[macro_export]
macro_rules! admin_source {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident;
        $($module:literal => $entity:path),* $(,)?
    ) => {
        $(#[$meta])*
        $vis struct $name {
            db: $crate::brocanth_model::sea_orm::DatabaseConnection,
            entries: ::std::sync::Arc<::std::vec::Vec<$crate::brocanth_admin::discovery::RegistryEntry>>,
            /// How a request's principal becomes its database identity.
            identity: ::std::option::Option<$crate::Identify>,
            /// The identity of the request this copy serves.
            me: ::std::option::Option<$crate::DbIdentity>,
        }

        impl $name {
            /// The source, over this connection.
            ///
            /// The columns are read here, once, because an entity's schema
            /// is in the binary and cannot change while the process runs.
            $vis fn new(db: $crate::brocanth_model::sea_orm::DatabaseConnection) -> Self {
                use $crate::brocanth_model::sea_orm::ConnectionTrait as _;
                let backend = db.get_database_backend();
                let entries = ::std::vec![
                    $($crate::entry_of::<$entity>($module, backend),)*
                ];
                Self {
                    db,
                    entries: ::std::sync::Arc::new(entries),
                    identity: ::std::option::Option::None,
                    me: ::std::option::Option::None,
                }
            }

            /// The source with no connection, for `backend`: the models and
            /// their columns, and no row. An admin built over it checks its
            /// configuration before the database is open, at start. A read
            /// or a write through it fails.
            #[allow(dead_code)]
            $vis fn unconnected(backend: $crate::brocanth_model::sea_orm::DbBackend) -> Self {
                let entries = ::std::vec![
                    $($crate::entry_of::<$entity>($module, backend),)*
                ];
                Self {
                    db: ::std::default::Default::default(),
                    entries: ::std::sync::Arc::new(entries),
                    identity: ::std::option::Option::None,
                    me: ::std::option::Option::None,
                }
            }

            /// Runs each request as the database identity `who` builds from
            /// the request's principal: each read and each write in a
            /// transaction that assumed it. For tables under Postgres
            /// row-level security.
            #[allow(dead_code)]
            $vis fn as_identity(
                mut self,
                who: impl for<'p> Fn($crate::brocanth_admin::Principal<'p>) -> $crate::DbIdentity
                    + ::std::marker::Send
                    + ::std::marker::Sync
                    + 'static,
            ) -> Self {
                self.identity = ::std::option::Option::Some(::std::sync::Arc::new(who));
                self
            }

            /// The connection, for a caller that wants to share it.
            $vis fn connection(&self) -> &$crate::brocanth_model::sea_orm::DatabaseConnection {
                &self.db
            }
        }

        // In a block, so the transaction type needs no name of its own and
        // two sources in one module do not collide.
        const _: () = {
        /// The writes of one admin save, in one database transaction.
        struct Transaction {
            txn: $crate::brocanth_model::sea_orm::DatabaseTransaction,
        }

        /// Each operation once, over any connection: the source's own, or
        /// a transaction that assumed the request's identity.
        impl $name {
            async fn count_on<C>(
                db: &C,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                search: $crate::brocanth_admin::data::Search<'_>,
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<usize, $crate::brocanth_admin::data::DataError>
            where
                C: $crate::brocanth_model::sea_orm::ConnectionTrait,
            {
                match model.module_path.as_str() {
                    $($module => $crate::count_of::<$entity>(db, search, scope).await,)*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
                }
            }

            async fn list_on<C>(
                db: &C,
                request: &$crate::brocanth_admin::data::ListRequest<'_>,
            ) -> ::std::result::Result<
                ::std::vec::Vec<$crate::brocanth_admin::model::Row>,
                $crate::brocanth_admin::data::DataError,
            >
            where
                C: $crate::brocanth_model::sea_orm::ConnectionTrait,
            {
                match request.model.module_path.as_str() {
                    $($module => $crate::rows_of::<$entity>(db, request).await,)*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
                }
            }

            async fn fetch_on<C>(
                db: &C,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<
                ::std::option::Option<$crate::brocanth_admin::model::Row>,
                $crate::brocanth_admin::data::DataError,
            >
            where
                C: $crate::brocanth_model::sea_orm::ConnectionTrait,
            {
                match model.module_path.as_str() {
                    $($module => $crate::row_of::<$entity>(db, key, &model.table, scope).await,)*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
                }
            }

            async fn fetch_many_on<C>(
                db: &C,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                column: &str,
                keys: &[::std::string::String],
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<
                ::std::vec::Vec<$crate::brocanth_admin::model::Row>,
                $crate::brocanth_admin::data::DataError,
            >
            where
                C: $crate::brocanth_model::sea_orm::ConnectionTrait,
            {
                match model.module_path.as_str() {
                    $($module => $crate::rows_by::<$entity>(db, column, keys, scope).await,)*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
                }
            }

            async fn insert_on<C>(
                db: &C,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                values: &[(::std::string::String, $crate::brocanth_admin::model::Value)],
            ) -> ::std::result::Result<
                $crate::brocanth_admin::model::PrimaryKey,
                $crate::brocanth_admin::data::DataError,
            >
            where
                C: $crate::brocanth_model::sea_orm::ConnectionTrait
                    + $crate::brocanth_model::sea_orm::TransactionTrait,
            {
                match model.module_path.as_str() {
                    $($module => $crate::insert_of::<$entity, _>(db, model, values).await,)*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
                }
            }

            async fn update_locked_on<C>(
                db: &C,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                values: &[(::std::string::String, $crate::brocanth_admin::model::Value)],
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<
                $crate::brocanth_admin::data::Locked,
                $crate::brocanth_admin::data::DataError,
            >
            where
                C: $crate::brocanth_model::sea_orm::ConnectionTrait
                    + $crate::brocanth_model::sea_orm::TransactionTrait,
            {
                match model.module_path.as_str() {
                    $($module => {
                        $crate::update_locked_of::<$entity, _>(db, model, key, values, scope)
                            .await
                    })*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
                }
            }

            async fn delete_locked_on<C>(
                db: &C,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<
                $crate::brocanth_admin::data::Locked,
                $crate::brocanth_admin::data::DataError,
            >
            where
                C: $crate::brocanth_model::sea_orm::ConnectionTrait
                    + $crate::brocanth_model::sea_orm::TransactionTrait,
            {
                match model.module_path.as_str() {
                    $($module => {
                        $crate::delete_locked_of::<$entity, _>(db, model, key, scope).await
                    })*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
                }
            }
        }

        #[$crate::async_trait]
        impl $crate::brocanth_admin::data::AdminTransaction for Transaction {
            async fn insert(
                &mut self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                values: &[(::std::string::String, $crate::brocanth_admin::model::Value)],
            ) -> ::std::result::Result<
                $crate::brocanth_admin::model::PrimaryKey,
                $crate::brocanth_admin::data::DataError,
            > {
                $name::insert_on(&self.txn, model, values).await
            }

            async fn update(
                &mut self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                values: &[(::std::string::String, $crate::brocanth_admin::model::Value)],
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<u64, $crate::brocanth_admin::data::DataError> {
                $name::update_locked_on(&self.txn, model, key, values, scope)
                    .await
                    .map(|locked| locked.rows)
            }

            async fn delete(
                &mut self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<u64, $crate::brocanth_admin::data::DataError> {
                $name::delete_locked_on(&self.txn, model, key, scope)
                    .await
                    .map(|locked| locked.rows)
            }

            async fn update_locked(
                &mut self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                values: &[(::std::string::String, $crate::brocanth_admin::model::Value)],
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<
                $crate::brocanth_admin::data::Locked,
                $crate::brocanth_admin::data::DataError,
            > {
                $name::update_locked_on(&self.txn, model, key, values, scope).await
            }

            async fn delete_locked(
                &mut self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<
                $crate::brocanth_admin::data::Locked,
                $crate::brocanth_admin::data::DataError,
            > {
                $name::delete_locked_on(&self.txn, model, key, scope).await
            }

            async fn commit(
                self: ::std::boxed::Box<Self>,
            ) -> ::std::result::Result<(), $crate::brocanth_admin::data::DataError> {
                self.txn.commit().await.map_err($crate::db_error)
            }

            async fn rollback(
                self: ::std::boxed::Box<Self>,
            ) -> ::std::result::Result<(), $crate::brocanth_admin::data::DataError> {
                self.txn.rollback().await.map_err($crate::db_error)
            }
        }

        #[$crate::async_trait]
        impl $crate::brocanth_admin::data::AdminData for $name {
            fn check(&self) -> ::std::result::Result<(), $crate::brocanth_admin::data::DataError> {
                match &self.identity {
                    ::std::option::Option::Some(who) => $crate::check_identity(who),
                    ::std::option::Option::None => ::std::result::Result::Ok(()),
                }
            }

            fn for_request(
                &self,
                cx: &$crate::brocanth_admin::data::RequestCx<'_>,
            ) -> ::std::option::Option<$crate::brocanth_admin::SharedData> {
                let who = self.identity.as_ref()?;
                ::std::option::Option::Some(::std::sync::Arc::new(Self {
                    db: self.db.clone(),
                    entries: ::std::sync::Arc::clone(&self.entries),
                    identity: ::std::option::Option::None,
                    me: ::std::option::Option::Some(who(cx.principal)),
                }))
            }

            async fn begin(
                &self,
            ) -> ::std::result::Result<
                ::std::boxed::Box<dyn $crate::brocanth_admin::data::AdminTransaction + '_>,
                $crate::brocanth_admin::data::DataError,
            > {
                use $crate::brocanth_model::sea_orm::TransactionTrait as _;
                let txn = match &self.me {
                    ::std::option::Option::Some(me) => $crate::begin_as(&self.db, me).await?,
                    ::std::option::Option::None => {
                        self.db.begin().await.map_err($crate::db_error)?
                    }
                };
                ::std::result::Result::Ok(::std::boxed::Box::new(Transaction { txn }))
            }

            fn registry(
                &self,
            ) -> ::std::vec::Vec<$crate::brocanth_admin::discovery::RegistryEntry> {
                (*self.entries).clone()
            }

            async fn count(
                &self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                search: $crate::brocanth_admin::data::Search<'_>,
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<usize, $crate::brocanth_admin::data::DataError> {
                match &self.me {
                    ::std::option::Option::None => Self::count_on(&self.db, model, search, scope).await,
                    ::std::option::Option::Some(me) => {
                        let txn = $crate::begin_as(&self.db, me).await?;
                        let out = Self::count_on(&txn, model, search, scope).await;
                        $crate::end(txn, out).await
                    }
                }
            }

            async fn list(
                &self,
                request: $crate::brocanth_admin::data::ListRequest<'_>,
            ) -> ::std::result::Result<
                ::std::vec::Vec<$crate::brocanth_admin::model::Row>,
                $crate::brocanth_admin::data::DataError,
            > {
                match &self.me {
                    ::std::option::Option::None => Self::list_on(&self.db, &request).await,
                    ::std::option::Option::Some(me) => {
                        let txn = $crate::begin_as(&self.db, me).await?;
                        let out = Self::list_on(&txn, &request).await;
                        $crate::end(txn, out).await
                    }
                }
            }

            async fn fetch(
                &self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<
                ::std::option::Option<$crate::brocanth_admin::model::Row>,
                $crate::brocanth_admin::data::DataError,
            > {
                match &self.me {
                    ::std::option::Option::None => Self::fetch_on(&self.db, model, key, scope).await,
                    ::std::option::Option::Some(me) => {
                        let txn = $crate::begin_as(&self.db, me).await?;
                        let out = Self::fetch_on(&txn, model, key, scope).await;
                        $crate::end(txn, out).await
                    }
                }
            }

            async fn fetch_many(
                &self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                column: &str,
                keys: &[::std::string::String],
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<
                ::std::vec::Vec<$crate::brocanth_admin::model::Row>,
                $crate::brocanth_admin::data::DataError,
            > {
                match &self.me {
                    ::std::option::Option::None => Self::fetch_many_on(&self.db, model, column, keys, scope).await,
                    ::std::option::Option::Some(me) => {
                        let txn = $crate::begin_as(&self.db, me).await?;
                        let out = Self::fetch_many_on(&txn, model, column, keys, scope).await;
                        $crate::end(txn, out).await
                    }
                }
            }

            async fn insert(
                &self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                values: &[(::std::string::String, $crate::brocanth_admin::model::Value)],
            ) -> ::std::result::Result<
                $crate::brocanth_admin::model::PrimaryKey,
                $crate::brocanth_admin::data::DataError,
            > {
                match &self.me {
                    ::std::option::Option::None => Self::insert_on(&self.db, model, values).await,
                    ::std::option::Option::Some(me) => {
                        let txn = $crate::begin_as(&self.db, me).await?;
                        let out = Self::insert_on(&txn, model, values).await;
                        $crate::end(txn, out).await
                    }
                }
            }

            async fn update(
                &self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                values: &[(::std::string::String, $crate::brocanth_admin::model::Value)],
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<u64, $crate::brocanth_admin::data::DataError> {
                match &self.me {
                    ::std::option::Option::None => Self::update_locked_on(&self.db, model, key, values, scope).await,
                    ::std::option::Option::Some(me) => {
                        let txn = $crate::begin_as(&self.db, me).await?;
                        let out = Self::update_locked_on(&txn, model, key, values, scope).await;
                        $crate::end(txn, out).await
                    }
                }
                    .map(|locked| locked.rows)
            }

            async fn delete(
                &self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<u64, $crate::brocanth_admin::data::DataError> {
                match &self.me {
                    ::std::option::Option::None => Self::delete_locked_on(&self.db, model, key, scope).await,
                    ::std::option::Option::Some(me) => {
                        let txn = $crate::begin_as(&self.db, me).await?;
                        let out = Self::delete_locked_on(&txn, model, key, scope).await;
                        $crate::end(txn, out).await
                    }
                }
                    .map(|locked| locked.rows)
            }

            async fn update_locked(
                &self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                values: &[(::std::string::String, $crate::brocanth_admin::model::Value)],
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<
                $crate::brocanth_admin::data::Locked,
                $crate::brocanth_admin::data::DataError,
            > {
                match &self.me {
                    ::std::option::Option::None => Self::update_locked_on(&self.db, model, key, values, scope).await,
                    ::std::option::Option::Some(me) => {
                        let txn = $crate::begin_as(&self.db, me).await?;
                        let out = Self::update_locked_on(&txn, model, key, values, scope).await;
                        $crate::end(txn, out).await
                    }
                }
            }

            async fn delete_locked(
                &self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<
                $crate::brocanth_admin::data::Locked,
                $crate::brocanth_admin::data::DataError,
            > {
                match &self.me {
                    ::std::option::Option::None => Self::delete_locked_on(&self.db, model, key, scope).await,
                    ::std::option::Option::Some(me) => {
                        let txn = $crate::begin_as(&self.db, me).await?;
                        let out = Self::delete_locked_on(&txn, model, key, scope).await;
                        $crate::end(txn, out).await
                    }
                }
            }
        }
        };
    };
}
