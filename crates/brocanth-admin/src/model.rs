//! What the admin knows about a model, a column, a value and a row.
//!
//! None of it comes from SeaORM directly. [`crate::data::AdminData`] hands
//! these over, and [`crate::discovery`] builds them from whatever the
//! application's registry reports — so this crate compiles, renders and is
//! tested with no SQL driver in its tree at all.

use std::fmt;

/// The coarse shape of a column, which is what decides a form widget.
///
/// SeaORM's `Schema::json_schema_from_entity` emits a type *name* per column.
/// [`ColumnKind::from_type_name`] folds that spelling into this set; anything
/// unrecognised lands on [`ColumnKind::Unknown`] and gets a text input, which
/// is the safe answer rather than a wrong one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ColumnKind {
    /// A short string: `char`, `string`, `var_char`.
    Text,
    /// A long string: `text`. Gets a textarea.
    LongText,
    /// Any width of integer.
    Integer,
    /// `float`, `double`.
    Float,
    /// `decimal`, `money`.
    Decimal,
    /// `boolean`.
    Boolean,
    /// `date`.
    Date,
    /// `time`.
    Time,
    /// `date_time`, `timestamp`, `timestamp_with_time_zone`.
    DateTime,
    /// `uuid`.
    Uuid,
    /// `json`, `json_binary`.
    Json,
    /// `binary`, `var_binary`, `blob`. Never editable in a form.
    Binary,
    /// A database enum. The variants live on [`ColumnDescriptor::variants`].
    Enumeration,
    /// A spelling this crate does not know.
    Unknown,
}

impl ColumnKind {
    /// Every variant, so a test can assert a widget exists for each one.
    pub const ALL: &'static [ColumnKind] = &[
        ColumnKind::Text,
        ColumnKind::LongText,
        ColumnKind::Integer,
        ColumnKind::Float,
        ColumnKind::Decimal,
        ColumnKind::Boolean,
        ColumnKind::Date,
        ColumnKind::Time,
        ColumnKind::DateTime,
        ColumnKind::Uuid,
        ColumnKind::Json,
        ColumnKind::Binary,
        ColumnKind::Enumeration,
        ColumnKind::Unknown,
    ];

    /// Folds a SeaORM column-type spelling into a kind.
    ///
    /// Case and separators are ignored, so `DateTime`, `date_time` and
    /// `datetime` all arrive at [`ColumnKind::DateTime`]. An unknown spelling
    /// is [`ColumnKind::Unknown`] rather than a panic: a new SeaORM column
    /// type must not take the admin down.
    pub fn from_type_name(name: &str) -> ColumnKind {
        let flat: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        match flat.as_str() {
            "char" | "string" | "varchar" => ColumnKind::Text,
            "text" => ColumnKind::LongText,
            "tinyinteger" | "smallinteger" | "integer" | "biginteger" | "tinyunsigned"
            | "smallunsigned" | "unsigned" | "bigunsigned" | "int" | "bigint" | "smallint"
            | "year" => ColumnKind::Integer,
            "float" | "double" | "real" => ColumnKind::Float,
            "decimal" | "money" | "numeric" => ColumnKind::Decimal,
            "boolean" | "bool" => ColumnKind::Boolean,
            "date" => ColumnKind::Date,
            "time" => ColumnKind::Time,
            "datetime" | "timestamp" | "timestampwithtimezone" | "timestamptz" => {
                ColumnKind::DateTime
            }
            "uuid" => ColumnKind::Uuid,
            "json" | "jsonbinary" | "jsonb" => ColumnKind::Json,
            "binary" | "varbinary" | "blob" | "bit" | "varbit" => ColumnKind::Binary,
            "enum" | "enumeration" => ColumnKind::Enumeration,
            _ => ColumnKind::Unknown,
        }
    }

    /// Whether a value of this kind is a number, which decides both the input
    /// type and the coercion applied to what was typed.
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            ColumnKind::Integer | ColumnKind::Float | ColumnKind::Decimal
        )
    }
}

/// Where a foreign key points, and how to label what it points at.
///
/// SeaORM reports columns and relations through two different APIs, so this
/// does not fall out of `json_schema_from_entity`. It is supplied by the data
/// source, or declared on the model's configuration with
/// [`crate::config::ModelAdmin::foreign_key`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    /// The key of the model this column points at.
    pub target: String,
    /// The column on the target that this one matches, usually its primary key.
    pub target_column: String,
}

impl Relation {
    /// A relation to `target`'s column `target_column`.
    pub fn new(target: impl Into<String>, target_column: impl Into<String>) -> Self {
        Relation {
            target: target.into(),
            target_column: target_column.into(),
        }
    }
}

/// One column, as the admin needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnDescriptor {
    /// Column name as the database spells it.
    pub name: String,
    /// The coarse type.
    pub kind: ColumnKind,
    /// Whether the column accepts `NULL`.
    pub nullable: bool,
    /// Whether the column carries a unique constraint.
    pub unique: bool,
    /// Whether the column is part of the primary key.
    pub primary_key: bool,
    /// Whether the database fills the column in, so a create form must not.
    pub auto_generated: bool,
    /// Character limit, when the schema states one.
    pub max_length: Option<u32>,
    /// Enum variants, when [`ColumnKind::Enumeration`].
    pub variants: Vec<String>,
    /// Where this column points, when it is a foreign key.
    pub relation: Option<Relation>,
}

impl ColumnDescriptor {
    /// A column of `kind`, not null, not unique, not a key.
    pub fn new(name: impl Into<String>, kind: ColumnKind) -> Self {
        ColumnDescriptor {
            name: name.into(),
            kind,
            nullable: false,
            unique: false,
            primary_key: false,
            auto_generated: false,
            max_length: None,
            variants: Vec::new(),
            relation: None,
        }
    }

    /// Marks the column nullable.
    pub fn nullable(mut self) -> Self {
        self.nullable = true;
        self
    }

    /// Marks the column unique.
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Marks the column a database-generated primary key.
    pub fn primary_key(mut self) -> Self {
        self.primary_key = true;
        self.auto_generated = true;
        self
    }

    /// Sets a character limit.
    pub fn max_length(mut self, max: u32) -> Self {
        self.max_length = Some(max);
        self
    }

    /// Declares the enum variants, and sets the kind to match.
    pub fn variants<I, S>(mut self, variants: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.variants = variants.into_iter().map(Into::into).collect();
        self.kind = ColumnKind::Enumeration;
        self
    }

    /// Declares the column a foreign key.
    pub fn references(mut self, target: impl Into<String>, column: impl Into<String>) -> Self {
        self.relation = Some(Relation::new(target, column));
        self
    }

    /// A human label for the column: `author_id` becomes `Author`.
    ///
    /// The `_id` suffix is dropped for a foreign key because the field shows a
    /// record, not a number, and labelling it "Author id" beside a name is the
    /// small lie that makes an admin feel generated.
    pub fn label(&self) -> String {
        let base = if self.relation.is_some() {
            self.name.strip_suffix("_id").unwrap_or(&self.name)
        } else {
            &self.name
        };
        humanise(base)
    }
}

/// One model, as the admin needs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelDescriptor {
    /// The slug this model is addressed by in a URL, and the key every other
    /// part of the admin refers to it with.
    pub key: String,
    /// Rust module path, as the entity registry reports it. Used to filter
    /// discovery and shown in the index.
    pub module_path: String,
    /// Table name.
    pub table: String,
    /// Columns, in declaration order.
    pub columns: Vec<ColumnDescriptor>,
}

impl ModelDescriptor {
    /// A model with no columns.
    pub fn new(key: impl Into<String>, table: impl Into<String>) -> Self {
        let key = key.into();
        ModelDescriptor {
            module_path: String::new(),
            table: table.into(),
            key,
            columns: Vec::new(),
        }
    }

    /// Adds a column.
    pub fn with(mut self, column: ColumnDescriptor) -> Self {
        self.columns.push(column);
        self
    }

    /// Sets the module path.
    pub fn in_module(mut self, module_path: impl Into<String>) -> Self {
        self.module_path = module_path.into();
        self
    }

    /// A column by name.
    pub fn column(&self, name: &str) -> Option<&ColumnDescriptor> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// The primary-key columns, in declaration order.
    pub fn key_columns(&self) -> Vec<&ColumnDescriptor> {
        self.columns.iter().filter(|c| c.primary_key).collect()
    }

    /// A human name for the model: `blog_post` becomes `Blog post`.
    pub fn label(&self) -> String {
        humanise(&self.key)
    }
}

/// One stored value.
///
/// Deliberately small. An admin renders HTML and reads form posts, so a date
/// is text that the data source formatted and will parse back; adding a date
/// type here would mean choosing a date library, and §8 says the core of a
/// thing ships nothing it was not asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `NULL`.
    Null,
    /// Text, and anything the data source chose to spell as text.
    Text(String),
    /// A whole number.
    Int(i64),
    /// A fractional number.
    Float(f64),
    /// A boolean.
    Bool(bool),
}

impl Value {
    /// Whether this is `NULL`.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The value as a form field would carry it. `NULL` is the empty string.
    pub fn as_input(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Text(s) => s.clone(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Bool(b) => b.to_string(),
        }
    }

    /// Whether the value is truthy, for a checkbox.
    pub fn as_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Text(s) => matches!(s.as_str(), "true" | "1" | "t" | "yes" | "on"),
            _ => false,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // A cell is read by a person, so `NULL` is spelled, not blank:
            // an empty string and a null look identical in a table otherwise.
            Value::Null => f.write_str("—"),
            Value::Text(s) => f.write_str(s),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Bool(true) => f.write_str("yes"),
            Value::Bool(false) => f.write_str("no"),
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Text(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Text(s)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Int(i)
    }
}

impl From<i32> for Value {
    fn from(i: i32) -> Self {
        Value::Int(i64::from(i))
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

/// One record: column names to values, in column order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    values: Vec<(String, Value)>,
}

impl Row {
    /// An empty row.
    pub fn new() -> Self {
        Row::default()
    }

    /// Adds a value, replacing one already under that name.
    pub fn with(mut self, name: impl Into<String>, value: impl Into<Value>) -> Self {
        self.set(name, value);
        self
    }

    /// Sets a value, replacing one already under that name.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<Value>) {
        let name = name.into();
        let value = value.into();
        match self.values.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = value,
            None => self.values.push((name, value)),
        }
    }

    /// A value by column name.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.values.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// Every value, in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.values.iter().map(|(n, v)| (n.as_str(), v))
    }

    /// Whether the row holds nothing.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// How many values the row holds.
    pub fn len(&self) -> usize {
        self.values.len()
    }
}

/// One record's primary key, as text, because that is what a URL carries.
///
/// Composite keys are supported: the parts keep their column names, and the
/// URL form joins the values with a comma.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PrimaryKey {
    parts: Vec<(String, String)>,
}

impl PrimaryKey {
    /// A single-column key.
    pub fn single(column: impl Into<String>, value: impl fmt::Display) -> Self {
        PrimaryKey {
            parts: vec![(column.into(), value.to_string())],
        }
    }

    /// A key from named parts.
    pub fn from_parts<I, S, T>(parts: I) -> Self
    where
        I: IntoIterator<Item = (S, T)>,
        S: Into<String>,
        T: Into<String>,
    {
        PrimaryKey {
            parts: parts.into_iter().map(|(a, b)| (a.into(), b.into())).collect(),
        }
    }

    /// Reads the key out of a row, or `None` when a key column is missing.
    pub fn from_row(model: &ModelDescriptor, row: &Row) -> Option<Self> {
        let columns = model.key_columns();
        if columns.is_empty() {
            return None;
        }
        let mut parts = Vec::with_capacity(columns.len());
        for column in columns {
            let value = row.get(&column.name)?;
            parts.push((column.name.clone(), value.as_input()));
        }
        Some(PrimaryKey { parts })
    }

    /// Parses the URL form back into a key, given the model it belongs to.
    pub fn parse(model: &ModelDescriptor, segment: &str) -> Option<Self> {
        let columns = model.key_columns();
        if columns.is_empty() {
            return None;
        }
        let values: Vec<&str> = segment.split(',').collect();
        if values.len() != columns.len() || values.iter().any(|v| v.is_empty()) {
            return None;
        }
        Some(PrimaryKey {
            parts: columns
                .iter()
                .zip(values)
                .map(|(c, v)| (c.name.clone(), v.to_string()))
                .collect(),
        })
    }

    /// The URL form.
    pub fn to_segment(&self) -> String {
        self.parts
            .iter()
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The named parts.
    pub fn parts(&self) -> &[(String, String)] {
        &self.parts
    }

    /// One part by column name.
    pub fn get(&self, column: &str) -> Option<&str> {
        self.parts
            .iter()
            .find(|(n, _)| n == column)
            .map(|(_, v)| v.as_str())
    }
}

impl fmt::Display for PrimaryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_segment())
    }
}

/// `author_id` becomes `Author id`; `blog_post` becomes `Blog post`.
pub(crate) fn humanise(name: &str) -> String {
    let words = name.replace(['_', '-'], " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
