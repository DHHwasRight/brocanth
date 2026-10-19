//! The seam between the admin and whatever stores the rows.
//!
//! **This is the trait the model layer has to satisfy, and it is deliberately
//! the smallest one that can serve four screens**: enumerate models, describe
//! columns, count, list, fetch, insert, update, delete. Nothing in this crate
//! knows SeaORM exists. [`crate::fixture::Fixture`] implements the trait in
//! memory and is what every test here runs against; [`crate::adapter`] is the
//! one module an application fills in to reach a real database.
//!
//! The trait is `async` because an ORM is. It is object-safe through
//! `async_trait`, because the admin holds one `Arc<dyn AdminData>` and must
//! not be generic over it — a generic would reach the `Views` struct, then
//! every context type, then every template, and the override seam would stop
//! being a plain `Box<dyn View<_>>`.

use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;

use crate::discovery::RegistryEntry;
use crate::model::{ModelDescriptor, PrimaryKey, Row, Value};
use crate::query::Sort;

/// Field-level failures, as a form needs them.
///
/// The data source raises these when a write is rejected: the model layer's
/// validations run inside the write path (PLAN §7.1), so the admin only learns
/// a record is invalid by trying. Returning them in this shape is what lets
/// the form re-render with the message beside the field that caused it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FieldErrors {
    entries: Vec<(String, String)>,
    base: Vec<String>,
}

impl FieldErrors {
    /// No errors.
    pub fn new() -> Self {
        FieldErrors::default()
    }

    /// Records a message against a field.
    pub fn add(&mut self, field: impl Into<String>, message: impl Into<String>) {
        self.entries.push((field.into(), message.into()));
    }

    /// Records a message against the record as a whole.
    pub fn add_base(&mut self, message: impl Into<String>) {
        self.base.push(message.into());
    }

    /// Records a message against a field, by value.
    pub fn with(mut self, field: impl Into<String>, message: impl Into<String>) -> Self {
        self.add(field, message);
        self
    }

    /// Every message on one field.
    pub fn on<'a>(&'a self, field: &'a str) -> impl Iterator<Item = &'a str> {
        self.entries
            .iter()
            .filter(move |(f, _)| f == field)
            .map(|(_, m)| m.as_str())
    }

    /// Messages that belong to no single field.
    pub fn base(&self) -> &[String] {
        &self.base
    }

    /// Whether anything is wrong.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.base.is_empty()
    }
}

/// What a data source can fail with.
#[derive(Debug, Error)]
pub enum DataError {
    /// The key names no model this source holds.
    #[error("no model named `{0}`")]
    UnknownModel(String),
    /// The record is not valid. Carries the per-field messages.
    #[error("the record is not valid")]
    Invalid(FieldErrors),
    /// Anything the storage layer raised.
    #[error("{0}")]
    Backend(String),
}

impl DataError {
    /// A backend failure from any message.
    pub fn backend(message: impl Into<String>) -> Self {
        DataError::Backend(message.into())
    }

    /// The field errors, when this is a validation failure.
    pub fn field_errors(&self) -> Option<&FieldErrors> {
        match self {
            DataError::Invalid(errors) => Some(errors),
            _ => None,
        }
    }
}

/// A search term and the columns it applies to.
///
/// The columns are the model's configured searchable set, already checked
/// against the schema, so a data source may use them in a query directly.
#[derive(Clone, Copy, Debug)]
pub struct Search<'a> {
    /// The term. Empty means no search, and a source must then match every row.
    pub term: &'a str,
    /// The columns to match against.
    pub columns: &'a [String],
}

impl Search<'_> {
    /// Whether this search matches everything.
    pub fn is_empty(&self) -> bool {
        self.term.is_empty() || self.columns.is_empty()
    }
}

/// One page of a listing, as the data source is asked for it.
///
/// `offset` and `limit` are already clamped by [`crate::paging::Page`], so an
/// implementation never has to think about a page past the end.
#[derive(Clone, Copy, Debug)]
pub struct ListRequest<'a> {
    /// The model being listed.
    pub model: &'a ModelDescriptor,
    /// Rows to skip.
    pub offset: usize,
    /// Rows to return.
    pub limit: usize,
    /// The search to apply.
    pub search: Search<'a>,
    /// The sort to apply, already checked against the schema.
    pub sort: Option<&'a Sort>,
}

/// Everything the admin needs from the layer that stores rows.
#[async_trait]
pub trait AdminData: Send + Sync + 'static {
    /// Every model this source holds, before the admin's discovery filter.
    ///
    /// Synchronous, because an entity registry is read out of the binary and
    /// not out of a database. The migration table belongs in this list; it is
    /// [`crate::discovery`]'s job to drop it, not this one's.
    fn registry(&self) -> Vec<RegistryEntry>;

    /// How many rows match the search.
    async fn count(&self, model: &ModelDescriptor, search: Search<'_>) -> Result<usize, DataError>;

    /// One page of rows.
    async fn list(&self, request: ListRequest<'_>) -> Result<Vec<Row>, DataError>;

    /// One row, or `None` when the key matches nothing.
    async fn fetch(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
    ) -> Result<Option<Row>, DataError>;

    /// Writes a new row and returns its key.
    ///
    /// Raise [`DataError::Invalid`] rather than a backend error when the
    /// model layer rejects the record: that is what reaches the form as a
    /// message beside the field.
    async fn insert(
        &self,
        model: &ModelDescriptor,
        values: &[(String, Value)],
    ) -> Result<PrimaryKey, DataError>;

    /// Writes over an existing row.
    async fn update(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        values: &[(String, Value)],
    ) -> Result<(), DataError>;

    /// Removes a row.
    async fn delete(&self, model: &ModelDescriptor, key: &PrimaryKey) -> Result<(), DataError>;
}

/// A shared data source, which is how the admin holds one.
pub type SharedData = Arc<dyn AdminData>;
