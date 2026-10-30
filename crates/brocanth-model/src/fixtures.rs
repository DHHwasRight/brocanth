//! Fixtures: a known dataset, loaded from a file through the model layer.
//!
//! Django's `loaddata` (D27). A fixture is a JSON file of records. Each
//! record names a model and its column values:
//!
//! ```json
//! [
//!   { "model": "user", "fields": { "id": 1, "name": "Ada", "email": "ada@example.com" } },
//!   { "model": "post", "fields": { "id": 1, "user_id": 1, "title": "Hello",
//!                                  "slug": "hello", "status": "draft", "views": 0 } }
//! ]
//! ```
//!
//! # Typed against the entities
//!
//! A fixture names a model by its table name and a column by its database
//! name. Both are checked against the entities the application registered
//! with [`Fixtures::model`], **before any row is written**. A record that
//! names an unknown model, an unknown column, or a value that does not fit
//! its column's Rust type stops the load, and the error gives the file, the
//! line and the column of the mistake, plus a `help:` line:
//!
//! ```text
//! fixtures/blog.json:4:44: record 1 (`user`) has no column `emial`
//!   = help: did you mean `email`? The columns of `user` are: id, name, email
//! ```
//!
//! Every problem in the file is reported, not only the first.
//!
//! # Through the model layer
//!
//! Each record becomes the entity's `ActiveModel`, set column by column with
//! `try_set`, and is written with `insert`. So `before_save` runs, and with
//! it every rule `#[brocanth_model::validates]` declared: a fixture cannot
//! put a row in the database that the application itself would refuse.
//! Django and Rails both skip the model's hooks when they load a fixture.
//! This loader does not, on purpose: a seed that breaks a validation is a
//! seed that no screen can edit.
//!
//! Records are written in file order, one transaction for the whole file.
//! A row that refers to another row comes after it. If one record fails,
//! nothing from the file stays.
//!
//! # In a test
//!
//! [`Harness::begin`](crate::testing::Harness::begin) gives a transaction
//! that cannot commit. Load into it: the fixture's own transaction becomes a
//! `SAVEPOINT`, and every row goes away at the end of the test.
//!
//! ```ignore
//! let harness = Harness::migrated::<Migrator>().await?;
//! let txn = harness.begin().await?;
//! let fixtures = Fixtures::new()
//!     .dir(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures"))
//!     .model::<user::Entity>()
//!     .model::<post::Entity>();
//! fixtures.load("blog", &*txn).await?;
//! ```
//!
//! # From the command line
//!
//! `brocanth fixtures load blog` hands `fixtures load blog` to the project's
//! own binary, like every command `brocanth` does not define. The global CLI
//! cannot know a project's entities, so the command lives in the project.
//! `docs/fixtures.md` has the fifteen lines.
//!
//! # Postgres sequences
//!
//! A record that sets an integer primary key leaves the table's sequence
//! behind the rows. On Postgres the loader moves each sequence it touched to
//! the largest key, as Django does, so the next `INSERT` without a key does
//! not collide. SQLite needs nothing: its next key is always past the
//! largest.
//!
//! # Not here
//!
//! `dumpdata`. A JSON or array column, a `Decimal` and a `Uuid` cannot be
//! written from a fixture yet: their types live behind SeaORM features this
//! crate does not turn on, and the error says so.

use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use sea_orm::prelude::{Date, DateTime, DateTimeUtc, DateTimeWithTimeZone, Time};
use sea_orm::sea_query::{ColumnType, Value};
use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseTransaction, EntityTrait, IdenStatic, IntoActiveModel, Iterable, PrimaryKeyToColumn,
    Statement, TransactionTrait,
};
use serde_json::value::RawValue;
use serde_json::{Map, Value as Json};

use crate::Error;

/// The extension of a fixture file.
pub const EXTENSION: &str = "json";

/// One thing wrong with a fixture, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The file, as the caller named it.
    pub file: String,
    /// 1-indexed line.
    pub line: usize,
    /// 1-indexed column.
    pub column: usize,
    /// What is wrong.
    pub message: String,
    /// What to do about it.
    pub help: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}\n  = help: {}",
            self.file, self.line, self.column, self.message, self.help
        )
    }
}

/// Why a fixture did not load. Holds every problem found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureError {
    /// At least one.
    pub problems: Vec<Problem>,
}

impl FixtureError {
    fn one(problem: Problem) -> Self {
        Self {
            problems: vec![problem],
        }
    }
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, p) in self.problems.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "{p}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FixtureError {}

/// What a load wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Loaded {
    /// Rows written, per model, in the order each model first appeared.
    pub per_model: Vec<(String, usize)>,
}

impl Loaded {
    /// Rows written in total.
    pub fn rows(&self) -> usize {
        self.per_model.iter().map(|(_, n)| n).sum()
    }

    fn count(&mut self, model: &str) {
        match self.per_model.iter_mut().find(|(m, _)| m == model) {
            Some((_, n)) => *n += 1,
            None => self.per_model.push((model.to_owned(), 1)),
        }
    }
}

/// The entities a fixture may name, and where fixture files live.
///
/// Built by hand, like [`ModelRegistry`](crate::ModelRegistry): the set of
/// models a fixture can write is the set the application said.
pub struct Fixtures {
    dir: PathBuf,
    models: Vec<Box<dyn Model>>,
}

impl Default for Fixtures {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Fixtures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Fixtures")
            .field("dir", &self.dir)
            .field("models", &self.names())
            .finish()
    }
}

impl Fixtures {
    /// No models, and files under `fixtures/` in the working directory.
    pub fn new() -> Self {
        Self {
            dir: PathBuf::from("fixtures"),
            models: Vec::new(),
        }
    }

    /// Where [`Fixtures::load`] looks for `<name>.json`.
    #[must_use]
    pub fn dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dir = dir.into();
        self
    }

    /// Lets a fixture write `E`. A record names it by its table name.
    #[must_use]
    pub fn model<E>(mut self) -> Self
    where
        E: EntityTrait + Default,
        E::ActiveModel: ActiveModelTrait<Entity = E> + ActiveModelBehavior + Send + Sync,
        E::Model: IntoActiveModel<E::ActiveModel>,
    {
        self.models.push(Box::new(Typed::<E>(PhantomData)));
        self
    }

    /// The model names a fixture may use, in registration order.
    pub fn names(&self) -> Vec<&str> {
        self.models.iter().map(|m| m.table()).collect()
    }

    /// The fixture files in [`Fixtures::dir`], by name, sorted.
    pub fn available(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(EXTENSION))
            .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
            .collect();
        out.sort();
        out
    }

    /// The file a name resolves to: `<dir>/<name>.json`, or the name itself
    /// when it already ends in `.json`.
    pub fn path_of(&self, name: &str) -> PathBuf {
        if Path::new(name).extension().and_then(|e| e.to_str()) == Some(EXTENSION) {
            PathBuf::from(name)
        } else {
            self.dir.join(format!("{name}.{EXTENSION}"))
        }
    }

    /// Checks a fixture against the entities without a database.
    ///
    /// Returns the number of records. This is the first half of every load,
    /// and it is public so a test or a CI step can check every fixture file
    /// without a database.
    pub fn check(&self, file: &str, text: &str) -> Result<usize, FixtureError> {
        self.parse(file, text).map(|records| records.len())
    }

    /// Reads `<dir>/<name>.json` and loads it. See [`Fixtures::load_str`].
    pub async fn load<C>(&self, name: &str, db: &C) -> Result<Loaded, FixtureError>
    where
        C: TransactionTrait<Transaction = DatabaseTransaction> + ConnectionTrait,
    {
        let path = self.path_of(name);
        let label = path.display().to_string();
        let text = std::fs::read_to_string(&path).map_err(|e| {
            let available = self.available();
            FixtureError::one(Problem {
                file: label.clone(),
                line: 1,
                column: 1,
                message: format!("cannot read the fixture `{name}`: {e}"),
                help: if available.is_empty() {
                    format!(
                        "create `{label}`, or point `Fixtures::dir` at the directory that holds it"
                    )
                } else {
                    format!(
                        "the fixtures in {} are: {}",
                        self.dir.display(),
                        available.join(", ")
                    )
                },
            })
        })?;
        self.load_str(&label, &text, db).await
    }

    /// Checks a fixture, then writes every record in one transaction.
    ///
    /// `file` only names the text in an error. On `db` being a
    /// [`TestTransaction`](crate::testing::TestTransaction), the transaction
    /// is a savepoint and nothing outlives the test.
    pub async fn load_str<C>(&self, file: &str, text: &str, db: &C) -> Result<Loaded, FixtureError>
    where
        C: TransactionTrait<Transaction = DatabaseTransaction> + ConnectionTrait,
    {
        let records = self.parse(file, text)?;
        let txn = db.begin().await.map_err(|e| {
            FixtureError::one(problem(
                file,
                1,
                1,
                format!("cannot open a transaction: {e}"),
                "check that the database is reachable",
            ))
        })?;

        let mut loaded = Loaded::default();
        let mut keyed: Vec<usize> = Vec::new();
        for record in &records {
            let model = &self.models[record.model];
            model
                .insert(&record.fields, &txn)
                .await
                .map_err(|e| FixtureError::one(record.refused(file, model.table(), e)))?;
            loaded.count(model.table());
            if record.sets_integer_key && !keyed.contains(&record.model) {
                keyed.push(record.model);
            }
        }

        if txn.get_database_backend() == DatabaseBackend::Postgres {
            for index in keyed {
                let model = &self.models[index];
                let Some(key) = model.integer_key() else {
                    continue;
                };
                let sql = format!(
                    "SELECT setval(pg_get_serial_sequence('\"{table}\"', '{key}'), \
                     (SELECT MAX(\"{key}\") FROM \"{table}\"))",
                    table = model.table()
                );
                txn.execute_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
                    .await
                    .map_err(|e| {
                        FixtureError::one(problem(
                            file,
                            1,
                            1,
                            format!("cannot move the sequence of `{}`: {e}", model.table()),
                            "the rows are not kept; check that the key column has a sequence",
                        ))
                    })?;
            }
        }

        txn.commit().await.map_err(|e| {
            FixtureError::one(problem(
                file,
                1,
                1,
                format!("cannot commit: {e}"),
                "nothing from the file was kept",
            ))
        })?;
        Ok(loaded)
    }

    /// The whole file, checked. Every problem is collected.
    fn parse(&self, file: &str, text: &str) -> Result<Vec<Record>, FixtureError> {
        let raws: Vec<&RawValue> = serde_json::from_str(text).map_err(|e| {
            FixtureError::one(problem(
                file,
                e.line().max(1),
                e.column().max(1),
                format!("the fixture is not a JSON array of records: {e}"),
                r#"a fixture is `[{"model": "<table>", "fields": {"<column>": <value>}}]`"#,
            ))
        })?;

        let mut problems = Vec::new();
        let mut records = Vec::new();
        for (index, raw) in raws.iter().enumerate() {
            let at = Span::of(text, raw.get());
            match self.record(file, index, raw, at) {
                Ok(record) => records.push(record),
                Err(mut found) => problems.append(&mut found),
            }
        }
        if problems.is_empty() {
            Ok(records)
        } else {
            Err(FixtureError { problems })
        }
    }

    fn record(
        &self,
        file: &str,
        index: usize,
        raw: &RawValue,
        at: Span<'_>,
    ) -> Result<Record, Vec<Problem>> {
        let n = index + 1;
        let here = |needle: &str| at.find(needle);
        let shape = r#"a record is `{"model": "<table>", "fields": {"<column>": <value>}}`"#;
        let Ok(Json::Object(object)) = serde_json::from_str::<Json>(raw.get()) else {
            let (line, column) = at.start();
            return Err(vec![problem(
                file,
                line,
                column,
                format!("record {n} is not an object"),
                shape,
            )]);
        };

        let mut problems = Vec::new();
        for key in object.keys() {
            if key != "model" && key != "fields" {
                let (line, column) = here(&format!("\"{key}\""));
                let help = if key == "pk" {
                    "put the primary key in `fields`, under its column name".to_owned()
                } else {
                    shape.to_owned()
                };
                problems.push(problem(
                    file,
                    line,
                    column,
                    format!("record {n} has an unknown key `{key}`"),
                    help,
                ));
            }
        }

        let Some(name) = object.get("model").and_then(Json::as_str) else {
            let (line, column) = at.start();
            problems.push(problem(
                file,
                line,
                column,
                format!("record {n} names no model"),
                shape,
            ));
            return Err(problems);
        };
        let Some(model) = self.models.iter().position(|m| m.table() == name) else {
            let (line, column) = here(&format!("\"{name}\""));
            let names = self.names();
            let help = match nearest(name, names.iter().copied()) {
                Some(guess) => format!(
                    "did you mean `{guess}`? The models are: {}",
                    names.join(", ")
                ),
                None if names.is_empty() => {
                    "no model is registered; add one with `Fixtures::model::<x::Entity>()`"
                        .to_owned()
                }
                None => format!(
                    "the models are: {}. Register another with `Fixtures::model::<x::Entity>()`",
                    names.join(", ")
                ),
            };
            problems.push(problem(
                file,
                line,
                column,
                format!("record {n} names an unknown model `{name}`"),
                help,
            ));
            return Err(problems);
        };

        let fields = match object.get("fields") {
            Some(Json::Object(fields)) => fields.clone(),
            Some(_) => {
                let (line, column) = here("\"fields\"");
                problems.push(problem(
                    file,
                    line,
                    column,
                    format!("record {n} has `fields` that is not an object"),
                    shape,
                ));
                return Err(problems);
            }
            None => {
                let (line, column) = at.start();
                problems.push(problem(
                    file,
                    line,
                    column,
                    format!("record {n} (`{name}`) has no `fields`"),
                    shape,
                ));
                return Err(problems);
            }
        };

        let typed = &self.models[model];
        for (column, value) in &fields {
            if let Err(mistake) = typed.check(column, value) {
                let (line, col) = here(&format!("\"{column}\""));
                problems.push(mistake.at(file, line, col, n, name));
            }
        }
        if !problems.is_empty() {
            return Err(problems);
        }

        let sets_integer_key = typed
            .integer_key()
            .is_some_and(|key| fields.get(key).is_some_and(Json::is_number));
        Ok(Record {
            model,
            fields,
            span_line: at.start().0,
            span_column: at.start().1,
            source: raw.get().to_owned(),
            sets_integer_key,
            number: n,
        })
    }
}

/// One record, checked.
struct Record {
    model: usize,
    fields: Map<String, Json>,
    span_line: usize,
    span_column: usize,
    source: String,
    sets_integer_key: bool,
    number: usize,
}

impl Record {
    /// The database's or the validator's refusal, located at the record or
    /// at the first field it names.
    fn refused(&self, file: &str, model: &str, error: Error) -> Problem {
        let n = self.number;
        match &error {
            Error::Invalid(errors) => {
                let field = errors
                    .iter()
                    .next()
                    .map(|e| e.field.clone())
                    .unwrap_or_default();
                let (line, column) = self.locate(&format!("\"{field}\""));
                Problem {
                    file: file.to_owned(),
                    line,
                    column,
                    message: format!(
                        "record {n} (`{model}`) fails validation: {}",
                        errors.full_messages().join("; ")
                    ),
                    help: "change the value in the fixture; the model's own rules refuse it"
                        .to_owned(),
                }
            }
            Error::Db(e) => {
                let text = e.to_string();
                let lower = text.to_lowercase();
                let help = if lower.contains("foreign key") {
                    "a row must come after the rows it refers to; move this record below them"
                } else if lower.contains("unique") || lower.contains("duplicate") {
                    "a row with this key or unique value exists; change it, or load into an empty database"
                } else if lower.contains("not null") {
                    "set every column that has no default and is not nullable"
                } else {
                    "the database refused this row; the message above is its own"
                };
                Problem {
                    file: file.to_owned(),
                    line: self.span_line,
                    column: self.span_column,
                    message: format!("record {n} (`{model}`) was refused: {text}"),
                    help: help.to_owned(),
                }
            }
        }
    }

    fn locate(&self, needle: &str) -> (usize, usize) {
        match self.source.find(needle) {
            Some(at) => {
                let before = &self.source[..at];
                let lines = before.matches('\n').count();
                let column = match before.rfind('\n') {
                    Some(nl) => at - nl,
                    None => self.span_column + at,
                };
                (self.span_line + lines, column)
            }
            None => (self.span_line, self.span_column),
        }
    }
}

/// A record's text inside the whole file, for line numbers.
#[derive(Clone, Copy)]
struct Span<'a> {
    text: &'a str,
    offset: usize,
    len: usize,
}

impl<'a> Span<'a> {
    /// `part` is a slice of `text`, which is what `RawValue` hands back.
    fn of(text: &'a str, part: &'a str) -> Self {
        let offset = (part.as_ptr() as usize).saturating_sub(text.as_ptr() as usize);
        Self {
            text,
            offset: offset.min(text.len()),
            len: part.len(),
        }
    }

    fn start(&self) -> (usize, usize) {
        line_col(self.text, self.offset)
    }

    /// The first occurrence of `needle` inside the record, or its start.
    fn find(&self, needle: &str) -> (usize, usize) {
        let end = (self.offset + self.len).min(self.text.len());
        match self.text[self.offset..end].find(needle) {
            Some(at) => line_col(self.text, self.offset + at),
            None => self.start(),
        }
    }
}

fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset];
    let line = before.matches('\n').count() + 1;
    let column = match before.rfind('\n') {
        Some(nl) => offset - nl,
        None => offset + 1,
    };
    (line, column)
}

fn problem(
    file: &str,
    line: usize,
    column: usize,
    message: impl Into<String>,
    help: impl Into<String>,
) -> Problem {
    Problem {
        file: file.to_owned(),
        line,
        column,
        message: message.into(),
        help: help.into(),
    }
}

/// A column problem, before it knows where it is.
struct Mistake {
    message: String,
    help: String,
}

impl Mistake {
    fn at(self, file: &str, line: usize, column: usize, record: usize, model: &str) -> Problem {
        problem(
            file,
            line,
            column,
            format!("record {record} (`{model}`) {}", self.message),
            self.help,
        )
    }
}

type Pending<'a> = Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>>;

/// One registered entity, with its type erased.
trait Model: Send + Sync {
    fn table(&self) -> &str;
    fn columns(&self) -> Vec<&'static str>;
    /// The primary key's column, when the key is one integer column.
    fn integer_key(&self) -> Option<&'static str>;
    fn check(&self, column: &str, value: &Json) -> Result<(), Mistake>;
    fn insert<'a>(
        &'a self,
        fields: &'a Map<String, Json>,
        txn: &'a DatabaseTransaction,
    ) -> Pending<'a>;
}

struct Typed<E>(PhantomData<fn() -> E>);

impl<E> Typed<E>
where
    E: EntityTrait + Default,
    E::ActiveModel: ActiveModelTrait<Entity = E>,
{
    fn column(name: &str) -> Option<E::Column> {
        E::Column::iter().find(|c| c.as_str() == name)
    }

    /// Sets one column on `active`, trying each value the JSON could mean
    /// until one fits the field's Rust type.
    fn set(active: &mut E::ActiveModel, column: E::Column, value: &Json) -> Result<(), Mistake> {
        let def = column.def();
        let ty = def.get_column_type();
        let candidates = candidates(ty, value).map_err(|reason| Mistake {
            message: format!("cannot set `{}`: {reason}", column.as_str()),
            help: "write this column in Rust after the load, or leave it to its default".to_owned(),
        })?;
        for candidate in candidates {
            if active.try_set(column, candidate).is_ok() {
                return Ok(());
            }
        }
        let help = if value.is_null() {
            format!("`{}` is not nullable; give it a value", column.as_str())
        } else {
            format!(
                "`{}` is {}; write a value of that type",
                column.as_str(),
                type_name(ty)
            )
        };
        Err(Mistake {
            message: format!(
                "has `{}` = {value}, which does not fit a {} column",
                column.as_str(),
                type_name(ty)
            ),
            help,
        })
    }
}

impl<E> Model for Typed<E>
where
    E: EntityTrait + Default,
    E::ActiveModel: ActiveModelTrait<Entity = E> + ActiveModelBehavior + Send + Sync,
    E::Model: IntoActiveModel<E::ActiveModel>,
{
    fn table(&self) -> &str {
        E::default().table_name()
    }

    fn columns(&self) -> Vec<&'static str> {
        E::Column::iter().map(|c| c.as_str()).collect()
    }

    fn integer_key(&self) -> Option<&'static str> {
        let keys: Vec<E::Column> = E::PrimaryKey::iter().map(|k| k.into_column()).collect();
        match keys.as_slice() {
            [key] if is_integer(key.def().get_column_type()) => Some(key.as_str()),
            _ => None,
        }
    }

    fn check(&self, column: &str, value: &Json) -> Result<(), Mistake> {
        let Some(col) = Self::column(column) else {
            let columns = self.columns();
            let listed = columns.join(", ");
            let table = self.table();
            return Err(Mistake {
                message: format!("has no column `{column}`"),
                help: match nearest(column, columns.iter().copied()) {
                    Some(guess) => {
                        format!("did you mean `{guess}`? The columns of `{table}` are: {listed}")
                    }
                    None => format!("the columns of `{table}` are: {listed}"),
                },
            });
        };
        let mut scratch = <E::ActiveModel as ActiveModelTrait>::default();
        Self::set(&mut scratch, col, value)
    }

    fn insert<'a>(
        &'a self,
        fields: &'a Map<String, Json>,
        txn: &'a DatabaseTransaction,
    ) -> Pending<'a> {
        Box::pin(async move {
            let mut active = <E::ActiveModel as ActiveModelTrait>::default();
            for (name, value) in fields {
                // Checked already: an unknown column or a bad value never
                // reaches this point.
                if let Some(column) = Self::column(name) {
                    Self::set(&mut active, column, value)
                        .map_err(|m| Error::Db(sea_orm::DbErr::Custom(m.message)))?;
                }
            }
            active.insert(txn).await.map(|_| ()).map_err(Error::from)
        })
    }
}

fn is_integer(ty: &ColumnType) -> bool {
    matches!(
        ty,
        ColumnType::TinyInteger
            | ColumnType::SmallInteger
            | ColumnType::Integer
            | ColumnType::BigInteger
            | ColumnType::TinyUnsigned
            | ColumnType::SmallUnsigned
            | ColumnType::Unsigned
            | ColumnType::BigUnsigned
    )
}

/// A short name for a column type, for a message.
fn type_name(ty: &ColumnType) -> &'static str {
    match ty {
        ColumnType::Char(_) | ColumnType::String(_) | ColumnType::Text => "text",
        t if is_integer(t) => "integer",
        ColumnType::Float | ColumnType::Double => "floating-point",
        ColumnType::Decimal(_) | ColumnType::Money(_) => "decimal",
        ColumnType::Boolean => "boolean",
        ColumnType::DateTime | ColumnType::Timestamp => "date-time",
        ColumnType::TimestampWithTimeZone => "date-time with a time zone",
        ColumnType::Date => "date",
        ColumnType::Time => "time",
        ColumnType::Json | ColumnType::JsonBinary => "JSON",
        ColumnType::Uuid => "UUID",
        ColumnType::Enum { .. } => "enum",
        _ => "non-scalar",
    }
}

/// Every [`Value`] the JSON could mean, most likely first.
///
/// `try_set` is strict about the variant: an `i32` field takes `Value::Int`
/// and nothing else. So a number becomes every integer variant it fits, and
/// the field's own type picks one.
fn candidates(ty: &ColumnType, value: &Json) -> Result<Vec<Value>, String> {
    if matches!(
        ty,
        ColumnType::Json
            | ColumnType::JsonBinary
            | ColumnType::Uuid
            | ColumnType::Decimal(_)
            | ColumnType::Money(_)
            | ColumnType::Array(_)
    ) {
        return Err(format!(
            "a {} column cannot be written from a fixture yet",
            type_name(ty)
        ));
    }
    Ok(match value {
        Json::Null => vec![
            Value::Bool(None),
            Value::TinyInt(None),
            Value::SmallInt(None),
            Value::Int(None),
            Value::BigInt(None),
            Value::TinyUnsigned(None),
            Value::SmallUnsigned(None),
            Value::Unsigned(None),
            Value::BigUnsigned(None),
            Value::Float(None),
            Value::Double(None),
            Value::String(None),
            Value::Char(None),
            Value::ChronoDate(None),
            Value::ChronoTime(None),
            Value::ChronoDateTime(None),
            Value::ChronoDateTimeUtc(None),
            Value::ChronoDateTimeWithTimeZone(None),
        ],
        Json::Bool(b) => vec![Value::Bool(Some(*b))],
        Json::Number(n) => {
            let mut out = Vec::new();
            if let Some(i) = n.as_i64() {
                out.extend(i32::try_from(i).ok().map(|v| Value::Int(Some(v))));
                out.push(Value::BigInt(Some(i)));
                out.extend(i16::try_from(i).ok().map(|v| Value::SmallInt(Some(v))));
                out.extend(i8::try_from(i).ok().map(|v| Value::TinyInt(Some(v))));
                out.extend(u32::try_from(i).ok().map(|v| Value::Unsigned(Some(v))));
                out.extend(u64::try_from(i).ok().map(|v| Value::BigUnsigned(Some(v))));
                out.extend(u16::try_from(i).ok().map(|v| Value::SmallUnsigned(Some(v))));
                out.extend(u8::try_from(i).ok().map(|v| Value::TinyUnsigned(Some(v))));
            } else if let Some(u) = n.as_u64() {
                out.push(Value::BigUnsigned(Some(u)));
            }
            if let Some(f) = n.as_f64() {
                out.push(Value::Double(Some(f)));
                #[allow(clippy::cast_possible_truncation)]
                out.push(Value::Float(Some(f as f32)));
            }
            out
        }
        Json::String(s) => {
            let mut out = vec![Value::String(Some(s.clone()))];
            let mut chars = s.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                out.push(Value::Char(Some(c)));
            }
            if let Ok(t) = DateTimeWithTimeZone::parse_from_rfc3339(s) {
                out.push(Value::ChronoDateTimeWithTimeZone(Some(t)));
                out.push(Value::ChronoDateTimeUtc(Some(DateTimeUtc::from(t))));
                out.push(Value::ChronoDateTime(Some(t.naive_utc())));
            }
            if let Some(t) = naive_date_time(s) {
                out.push(Value::ChronoDateTime(Some(t)));
                out.push(Value::ChronoDateTimeUtc(Some(t.and_utc())));
                out.push(Value::ChronoDateTimeWithTimeZone(Some(
                    t.and_utc().fixed_offset(),
                )));
            }
            if let Ok(d) = s.parse::<Date>() {
                out.push(Value::ChronoDate(Some(d)));
            }
            if let Ok(t) = s.parse::<Time>() {
                out.push(Value::ChronoTime(Some(t)));
            }
            out
        }
        Json::Array(_) | Json::Object(_) => {
            return Err("an array or an object is not a column value".to_owned())
        }
    })
}

fn naive_date_time(s: &str) -> Option<DateTime> {
    s.parse::<DateTime>()
        .ok()
        .or_else(|| DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").ok())
}

/// The candidate nearest to `wanted`, when it is near enough to be a typo.
fn nearest<'a>(wanted: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .map(|c| (distance(wanted, c), c))
        .filter(|(d, c)| *d <= 2.max(c.len() / 3))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(here + 1);
            prev = here;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typo_is_near_and_a_stranger_is_not() {
        let cols = ["id", "name", "email"];
        assert_eq!(nearest("emial", cols.iter().copied()), Some("email"));
        assert_eq!(nearest("password", cols.iter().copied()), None);
    }

    #[test]
    fn lines_and_columns_are_one_indexed() {
        let text = "[\n  {\"a\": 1}\n]";
        assert_eq!(line_col(text, 0), (1, 1));
        let at = text.find("{").unwrap();
        assert_eq!(line_col(text, at), (2, 3));
    }

    #[test]
    fn a_number_offers_every_integer_it_fits() {
        let c = candidates(&ColumnType::Integer, &serde_json::json!(5)).unwrap();
        assert!(c.contains(&Value::Int(Some(5))));
        assert!(c.contains(&Value::BigInt(Some(5))));
        let c = candidates(&ColumnType::Integer, &serde_json::json!(-1)).unwrap();
        assert!(!c.iter().any(|v| matches!(v, Value::Unsigned(_))));
    }

    #[test]
    fn a_json_column_says_it_is_not_supported() {
        let e = candidates(&ColumnType::Json, &serde_json::json!({})).unwrap_err();
        assert!(e.contains("JSON"), "{e}");
    }
}
