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
//! **No composite-key writes.** Reads of a composite key work; `update` and
//! `delete` need `find_by_id` with a tuple, which has no dynamic spelling.
//! A composite key is refused by name rather than half-supported.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use brocanth_admin::data::DataError;
use brocanth_admin::model::{PrimaryKey, Row, Value};

pub use brocanth_admin;
pub use brocanth_model;
pub use serde_json;

mod source;

pub use async_trait::async_trait;
pub use source::{
    columns_of, count_of, delete_of, entry_of, insert_of, json_of, order_of, row_of, rows_of,
    scope_of, search_of, single_key, update_of,
};

/// A module path the source was not given an entity for.
pub fn unknown_model(module_path: &str) -> DataError {
    DataError::UnknownModel(module_path.to_string())
}

/// What a name that is not a column gets, rather than a query.
pub fn unknown_column(name: &str) -> DataError {
    DataError::backend(format!(
        "`{name}` is not a column on this entity; the admin was asked to \
         sort or search on a name the schema does not have"
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
/// A text value that parses as JSON is passed through as JSON, which is how
/// a `jsonb` column survives a round trip through a textarea. Everything
/// else is a string, and SeaORM casts it to the column's type — or refuses,
/// which is a validation message rather than a panic.
pub fn json_from_values(values: &[(String, Value)]) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    for (name, value) in values {
        object.insert(name.clone(), json_from_value(value));
    }
    serde_json::Value::Object(object)
}

fn json_from_value(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Text(s) => match serde_json::from_str::<serde_json::Value>(s) {
            // Only a composite: `"7"` parses as a number and must stay the
            // string the form sent, or every text column holding digits
            // changes type on its way to the database.
            Ok(parsed) if parsed.is_object() || parsed.is_array() => parsed,
            _ => serde_json::Value::String(s.clone()),
        },
    }
}

/// The one key column, or a refusal naming the model.
pub fn one_key(key: &PrimaryKey, table: &str) -> Result<(String, String), DataError> {
    let parts = key.parts();
    match parts {
        [(column, value)] => Ok((column.clone(), value.clone())),
        [] => Err(DataError::backend(format!(
            "`{table}` has no primary key, so the admin cannot address a row of it"
        ))),
        _ => Err(DataError::backend(format!(
            "`{table}` has a composite primary key, which this data source \
             reads but cannot write: `find_by_id` needs a tuple and there is \
             no dynamic spelling of one"
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
/// # Why the match is written six times
///
/// Once per method, by the macro. A helper macro holding it once cannot be
/// written: a `macro_rules!` nested inside another cannot capture the outer
/// one's variables, and every arm names a different entity, so no function
/// can hold them either. Six expansions of a two-line match is the cost of
/// the ORM being static and the admin being dynamic.
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
            entries: ::std::vec::Vec<$crate::brocanth_admin::discovery::RegistryEntry>,
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
                Self { db, entries }
            }

            /// The connection, for a caller that wants to share it.
            $vis fn connection(&self) -> &$crate::brocanth_model::sea_orm::DatabaseConnection {
                &self.db
            }
        }

        #[$crate::async_trait]
        impl $crate::brocanth_admin::data::AdminData for $name {
            fn registry(
                &self,
            ) -> ::std::vec::Vec<$crate::brocanth_admin::discovery::RegistryEntry> {
                self.entries.clone()
            }

            async fn count(
                &self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                search: $crate::brocanth_admin::data::Search<'_>,
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<usize, $crate::brocanth_admin::data::DataError> {
                match model.module_path.as_str() {
                    $($module => $crate::count_of::<$entity>(&self.db, search, scope).await,)*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
                }
            }

            async fn list(
                &self,
                request: $crate::brocanth_admin::data::ListRequest<'_>,
            ) -> ::std::result::Result<
                ::std::vec::Vec<$crate::brocanth_admin::model::Row>,
                $crate::brocanth_admin::data::DataError,
            > {
                match request.model.module_path.as_str() {
                    $($module => $crate::rows_of::<$entity>(&self.db, &request).await,)*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
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
                match model.module_path.as_str() {
                    $($module => {
                        $crate::row_of::<$entity>(&self.db, key, &model.table, scope).await
                    })*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
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
                match model.module_path.as_str() {
                    $($module => {
                        $crate::insert_of::<$entity>(&self.db, model, values).await
                    })*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
                }
            }

            async fn update(
                &self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                values: &[(::std::string::String, $crate::brocanth_admin::model::Value)],
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<u64, $crate::brocanth_admin::data::DataError> {
                match model.module_path.as_str() {
                    $($module => {
                        $crate::update_of::<$entity>(&self.db, model, key, values, scope).await
                    })*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
                }
            }

            async fn delete(
                &self,
                model: &$crate::brocanth_admin::model::ModelDescriptor,
                key: &$crate::brocanth_admin::model::PrimaryKey,
                scope: $crate::brocanth_admin::data::Scope<'_>,
            ) -> ::std::result::Result<u64, $crate::brocanth_admin::data::DataError> {
                match model.module_path.as_str() {
                    $($module => {
                        $crate::delete_of::<$entity>(&self.db, model, key, scope).await
                    })*
                    other => ::std::result::Result::Err($crate::unknown_model(other)),
                }
            }
        }
    };
}
