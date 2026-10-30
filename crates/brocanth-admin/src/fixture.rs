//! An in-memory [`AdminData`], which is what this crate is tested against.
//!
//! PLAN §7 says SeaORM 2.0 is three months old and the schedule is the risk.
//! So the admin was built against a fixture rather than against a database:
//! every screen, every widget and every boundary in the pagination is
//! exercised here with no driver in the tree, and [`crate::adapter`] is the
//! only thing left to write when the model layer lands.
//!
//! It is a real implementation rather than a stub. It paginates, searches,
//! sorts, enforces its unique constraints and raises
//! [`DataError::Invalid`] the way a model layer with validations would, so a
//! screen that works here is a screen whose logic is right.

use std::sync::Mutex;

use async_trait::async_trait;

use crate::data::{AdminData, DataError, FieldErrors, ListRequest, Scope, Search};
use crate::discovery::RegistryEntry;
use crate::model::{ColumnDescriptor, ColumnKind, ModelDescriptor, PrimaryKey, Row, Value};
use crate::query::{Direction, Sort};

/// An in-memory data source.
#[derive(Debug, Default)]
pub struct Fixture {
    entries: Vec<RegistryEntry>,
    tables: Mutex<Vec<(String, Vec<Row>)>>,
}

impl Fixture {
    /// An empty fixture.
    pub fn new() -> Self {
        Fixture::default()
    }

    /// Adds a model, with no rows.
    pub fn model(mut self, entry: RegistryEntry) -> Self {
        self.tables
            .get_mut()
            .expect("fixture lock")
            .push((entry.table.clone(), Vec::new()));
        self.entries.push(entry);
        self
    }

    /// Adds a row to a table.
    pub fn row(self, table: &str, row: Row) -> Self {
        {
            let mut tables = self.tables.lock().expect("fixture lock");
            match tables.iter_mut().find(|(t, _)| t == table) {
                Some((_, rows)) => rows.push(row),
                None => tables.push((table.to_string(), vec![row])),
            }
        }
        self
    }

    /// How many rows a table holds.
    pub fn len(&self, table: &str) -> usize {
        self.tables
            .lock()
            .expect("fixture lock")
            .iter()
            .find(|(t, _)| t == table)
            .map_or(0, |(_, rows)| rows.len())
    }

    /// Whether a table holds nothing.
    pub fn is_empty(&self, table: &str) -> bool {
        self.len(table) == 0
    }

    /// A blog: users and posts, plus SeaORM's migration table.
    ///
    /// The migration table is here on purpose. It is in a real registry, and a
    /// fixture that leaves it out would let the discovery filter rot without
    /// anything noticing.
    pub fn blog() -> Fixture {
        let user = RegistryEntry::new("blog::models::user", "user")
            .with(ColumnDescriptor::new("id", ColumnKind::Integer).primary_key())
            .with(ColumnDescriptor::new("name", ColumnKind::Text).max_length(80))
            .with(ColumnDescriptor::new("email", ColumnKind::Text).unique())
            .with(ColumnDescriptor::new("bio", ColumnKind::LongText).nullable())
            .with(ColumnDescriptor::new("role", ColumnKind::Text).variants(["author", "editor"]))
            .with(ColumnDescriptor::new("active", ColumnKind::Boolean))
            .with(ColumnDescriptor::new("joined_at", ColumnKind::DateTime).nullable());

        let post = RegistryEntry::new("blog::models::post", "post")
            .with(ColumnDescriptor::new("id", ColumnKind::Integer).primary_key())
            .with(ColumnDescriptor::new("title", ColumnKind::Text).max_length(120))
            .with(ColumnDescriptor::new("body", ColumnKind::LongText))
            .with(ColumnDescriptor::new("author_id", ColumnKind::Integer).references("user", "id"))
            .with(ColumnDescriptor::new("published", ColumnKind::Boolean))
            .with(ColumnDescriptor::new("views", ColumnKind::Integer))
            .with(ColumnDescriptor::new("published_at", ColumnKind::Date).nullable());

        // What SeaORM's own migrator registers. The admin must never show it.
        let migrations =
            RegistryEntry::new("sea_orm_migration::seaql_migrations", "seaql_migrations")
                .with(ColumnDescriptor::new("version", ColumnKind::Text).primary_key())
                .with(ColumnDescriptor::new("applied_at", ColumnKind::Integer));

        Fixture::new()
            .model(user)
            .model(post)
            .model(migrations)
            .row(
                "user",
                Row::new()
                    .with("id", 1)
                    .with("name", "Ada Lovelace")
                    .with("email", "ada@example.com")
                    .with("bio", "Wrote the first program.")
                    .with("role", "editor")
                    .with("active", true)
                    .with("joined_at", "1843-10-01T00:00"),
            )
            .row(
                "user",
                Row::new()
                    .with("id", 2)
                    .with("name", "Grace Hopper")
                    .with("email", "grace@example.com")
                    .with("bio", Value::Null)
                    .with("role", "author")
                    .with("active", true)
                    .with("joined_at", Value::Null),
            )
            .row(
                "post",
                Row::new()
                    .with("id", 1)
                    .with("title", "On the Analytical Engine")
                    .with("body", "Notes.")
                    .with("author_id", 1)
                    .with("published", true)
                    .with("views", 1843)
                    .with("published_at", "1843-10-01"),
            )
            .row(
                "post",
                Row::new()
                    .with("id", 2)
                    .with("title", "Compilers")
                    .with("body", "A0 and after.")
                    .with("author_id", 2)
                    .with("published", false)
                    .with("views", 12)
                    .with("published_at", Value::Null),
            )
            .row(
                "seaql_migrations",
                Row::new()
                    .with("version", "m20260101_000001_create_user")
                    .with("applied_at", 1_767_225_600),
            )
    }

    /// Fills a table with `count` generated rows, for a pagination test.
    pub fn filled(mut self, table: &str, count: usize) -> Self {
        let rows: Vec<Row> = (1..=count)
            .map(|n| {
                Row::new()
                    .with("id", n as i64)
                    .with("title", format!("Post {n}"))
                    .with("body", "…")
                    .with("author_id", 1)
                    .with("published", n % 2 == 0)
                    .with("views", n as i64)
                    .with("published_at", Value::Null)
            })
            .collect();
        {
            let tables = self.tables.get_mut().expect("fixture lock");
            match tables.iter_mut().find(|(t, _)| t == table) {
                Some((_, slot)) => *slot = rows,
                None => tables.push((table.to_string(), rows)),
            }
        }
        self
    }

    fn matching(&self, model: &ModelDescriptor, search: Search<'_>) -> Vec<Row> {
        let tables = self.tables.lock().expect("fixture lock");
        let rows = tables
            .iter()
            .find(|(t, _)| *t == model.table)
            .map(|(_, rows)| rows.clone())
            .unwrap_or_default();
        if search.is_empty() {
            return rows;
        }
        let needle = search.term.to_lowercase();
        rows.into_iter()
            .filter(|row| {
                search.columns.iter().any(|column| {
                    row.get(column)
                        .map(|v| v.as_input().to_lowercase().contains(&needle))
                        .unwrap_or(false)
                })
            })
            .collect()
    }
}

fn compare(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Some(Value::Int(x)), Some(Value::Int(y))) => x.cmp(y),
        (Some(Value::Float(x)), Some(Value::Float(y))) => {
            x.partial_cmp(y).unwrap_or(Ordering::Equal)
        }
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x.cmp(y),
        (Some(x), Some(y)) => x.as_input().cmp(&y.as_input()),
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
    }
}

fn sorted(rows: &mut [Row], sort: Option<&Sort>) {
    let Some(sort) = sort else {
        return;
    };
    rows.sort_by(|a, b| {
        let ordering = compare(a.get(&sort.column), b.get(&sort.column));
        match sort.direction {
            Direction::Ascending => ordering,
            Direction::Descending => ordering.reverse(),
        }
    });
}

fn matches_key(model: &ModelDescriptor, row: &Row, key: &PrimaryKey) -> bool {
    model.key_columns().iter().all(
        |column| match (row.get(&column.name), key.get(&column.name)) {
            (Some(value), Some(wanted)) => value.as_input() == wanted,
            _ => false,
        },
    )
}

/// The fixture holds rows in memory and has no SQL engine, so it cannot
/// apply a row policy's condition. It refuses one loudly: answering as if the
/// condition were `TRUE` would show every row to everybody, which is the bug
/// row policies exist to prevent.
fn unscoped(scope: Scope<'_>) -> Result<(), DataError> {
    match scope {
        Scope::Unscoped => Ok(()),
        Scope::Where(_) => Err(DataError::backend(
            "the in-memory Fixture cannot apply a row policy's SQL condition; \
             test row policies against a real database",
        )),
    }
}

#[async_trait]
impl AdminData for Fixture {
    fn registry(&self) -> Vec<RegistryEntry> {
        self.entries.clone()
    }

    async fn count(
        &self,
        model: &ModelDescriptor,
        search: Search<'_>,
        scope: Scope<'_>,
    ) -> Result<usize, DataError> {
        unscoped(scope)?;
        Ok(self.matching(model, search).len())
    }

    async fn list(&self, request: ListRequest<'_>) -> Result<Vec<Row>, DataError> {
        unscoped(request.scope)?;
        let mut rows = self.matching(request.model, request.search);
        sorted(&mut rows, request.sort);
        Ok(rows
            .into_iter()
            .skip(request.offset)
            .take(request.limit)
            .collect())
    }

    async fn fetch(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: Scope<'_>,
    ) -> Result<Option<Row>, DataError> {
        unscoped(scope)?;
        let tables = self.tables.lock().expect("fixture lock");
        Ok(tables
            .iter()
            .find(|(t, _)| *t == model.table)
            .and_then(|(_, rows)| {
                rows.iter()
                    .find(|row| matches_key(model, row, key))
                    .cloned()
            }))
    }

    async fn insert(
        &self,
        model: &ModelDescriptor,
        values: &[(String, Value)],
    ) -> Result<PrimaryKey, DataError> {
        let mut tables = self.tables.lock().expect("fixture lock");
        let Some((_, rows)) = tables.iter_mut().find(|(t, _)| *t == model.table) else {
            return Err(DataError::UnknownModel(model.key.clone()));
        };

        // What a model layer with `uniqueness` validations would raise, in the
        // shape the form needs to put the message beside the field.
        let mut errors = FieldErrors::new();
        for column in model.columns.iter().filter(|c| c.unique) {
            let Some((_, submitted)) = values.iter().find(|(n, _)| *n == column.name) else {
                continue;
            };
            if rows
                .iter()
                .any(|row| row.get(&column.name).map(Value::as_input) == Some(submitted.as_input()))
            {
                errors.add(&column.name, "has already been taken");
            }
        }
        if !errors.is_empty() {
            return Err(DataError::Invalid(errors));
        }

        let mut row = Row::new();
        for column in &model.columns {
            if column.primary_key && column.auto_generated {
                let next = rows
                    .iter()
                    .filter_map(|r| r.get(&column.name).map(Value::as_input))
                    .filter_map(|v| v.parse::<i64>().ok())
                    .max()
                    .unwrap_or(0)
                    + 1;
                row.set(&column.name, Value::Int(next));
                continue;
            }
            match values.iter().find(|(n, _)| *n == column.name) {
                Some((_, value)) => row.set(&column.name, value.clone()),
                None => row.set(&column.name, Value::Null),
            }
        }
        let key = PrimaryKey::from_row(model, &row)
            .ok_or_else(|| DataError::backend("the fixture row has no primary key"))?;
        rows.push(row);
        Ok(key)
    }

    async fn update(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        values: &[(String, Value)],
        scope: Scope<'_>,
    ) -> Result<u64, DataError> {
        unscoped(scope)?;
        let mut tables = self.tables.lock().expect("fixture lock");
        let Some((_, rows)) = tables.iter_mut().find(|(t, _)| *t == model.table) else {
            return Err(DataError::UnknownModel(model.key.clone()));
        };
        let mut errors = FieldErrors::new();
        for column in model.columns.iter().filter(|c| c.unique) {
            let Some((_, submitted)) = values.iter().find(|(n, _)| *n == column.name) else {
                continue;
            };
            if rows.iter().any(|row| {
                !matches_key(model, row, key)
                    && row.get(&column.name).map(Value::as_input) == Some(submitted.as_input())
            }) {
                errors.add(&column.name, "has already been taken");
            }
        }
        if !errors.is_empty() {
            return Err(DataError::Invalid(errors));
        }
        // A missing row is zero rows changed, which the admin answers as a
        // lost race rather than as success.
        let Some(row) = rows.iter_mut().find(|row| matches_key(model, row, key)) else {
            return Ok(0);
        };
        for (name, value) in values {
            row.set(name, value.clone());
        }
        Ok(1)
    }

    async fn delete(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: Scope<'_>,
    ) -> Result<u64, DataError> {
        unscoped(scope)?;
        let mut tables = self.tables.lock().expect("fixture lock");
        let Some((_, rows)) = tables.iter_mut().find(|(t, _)| *t == model.table) else {
            return Err(DataError::UnknownModel(model.key.clone()));
        };
        let before = rows.len();
        rows.retain(|row| !matches_key(model, row, key));
        Ok((before - rows.len()) as u64)
    }
}
