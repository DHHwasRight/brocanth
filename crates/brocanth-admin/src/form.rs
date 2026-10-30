//! Forms derived from the schema, and the re-render that keeps what was typed.
//!
//! A column type picks a widget ([`widget_for`]), the model's configuration
//! may override it, and the whole form is a plain value a template walks.
//!
//! **The rule this module exists to enforce: a rejected submission re-renders
//! with the submitted values and the errors beside the fields that caused
//! them.** A form that loses what the user typed is the single most-hated bug
//! an admin can have, and it happens because the obvious implementation
//! rebuilds the form from the database row after a failed write. So the flow
//! here never does: [`Form::accept`] puts the submission into the fields,
//! validation and the data source only ever *add* errors, and nothing reads
//! the row again.

use crate::config::ModelAdmin;
use crate::data::FieldErrors;
use crate::model::{ColumnDescriptor, ColumnKind, ModelDescriptor, PrimaryKey, Row, Value};

/// How one field is edited.
///
/// Small and closed. A widget nobody can render is worse than a text input,
/// so an unknown column type lands on [`Widget::Text`] rather than on
/// something clever.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Widget {
    /// A single-line text input.
    Text,
    /// A multi-line text input, for a `text` column.
    Textarea,
    /// `<input type="number">`.
    Number,
    /// `<input type="checkbox">`.
    Checkbox,
    /// `<input type="date">`.
    Date,
    /// `<input type="time">`.
    Time,
    /// `<input type="datetime-local">`.
    DateTime,
    /// `<select>` over a database enum's variants.
    Select,
    /// `<select>` over another model's records, labelled rather than numbered.
    ForeignKey,
    /// A textarea that says it holds JSON.
    Json,
    /// Shown, never submitted. A generated key, or a column named read-only.
    ReadOnly,
    /// Not shown and not submitted.
    Hidden,
}

impl Widget {
    /// The `type` attribute of the `<input>` this widget renders, when it
    /// renders one.
    pub fn input_type(self) -> &'static str {
        match self {
            Widget::Number => "number",
            Widget::Checkbox => "checkbox",
            Widget::Date => "date",
            Widget::Time => "time",
            Widget::DateTime => "datetime-local",
            _ => "text",
        }
    }

    /// Whether a submission is read for this widget at all.
    pub fn is_editable(self) -> bool {
        !matches!(self, Widget::ReadOnly | Widget::Hidden)
    }
}

/// The widget a column gets from its type alone.
///
/// The order of the checks is the whole of the rule, and it is deliberate:
///
/// 1. a generated primary key is [`Widget::Hidden`] — the database fills it in
///    and the URL already carries it;
/// 2. a foreign key is [`Widget::ForeignKey`], whatever its column type is,
///    because an integer input pointing at another table is the thing this
///    screen exists to avoid;
/// 3. an enum is [`Widget::Select`];
/// 4. everything else follows [`ColumnKind`].
pub fn widget_for(column: &ColumnDescriptor) -> Widget {
    if column.primary_key && column.auto_generated {
        return Widget::Hidden;
    }
    if column.relation.is_some() {
        return Widget::ForeignKey;
    }
    if column.kind == ColumnKind::Enumeration || !column.variants.is_empty() {
        return Widget::Select;
    }
    match column.kind {
        ColumnKind::Text | ColumnKind::Uuid | ColumnKind::Unknown => Widget::Text,
        ColumnKind::LongText => Widget::Textarea,
        ColumnKind::Integer | ColumnKind::Float | ColumnKind::Decimal => Widget::Number,
        ColumnKind::Boolean => Widget::Checkbox,
        ColumnKind::Date => Widget::Date,
        ColumnKind::Time => Widget::Time,
        ColumnKind::DateTime => Widget::DateTime,
        ColumnKind::Json => Widget::Json,
        // A blob has no sensible text form, so showing one and refusing to
        // write it is the honest answer.
        ColumnKind::Binary => Widget::ReadOnly,
        ColumnKind::Enumeration => Widget::Select,
    }
}

/// One option in a `<select>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// What is submitted.
    pub value: String,
    /// What is shown.
    pub label: String,
}

impl Choice {
    /// A choice.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Choice {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// One editable field.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// The column name, and the form input's `name`.
    pub name: String,
    /// What the label says.
    pub label: String,
    /// How it is edited.
    pub widget: Widget,
    /// The column's type, which the template uses for the odd hint.
    pub kind: ColumnKind,
    /// What the input holds. **This is the submitted text after
    /// [`Form::accept`], not a value read back from the database.**
    pub value: String,
    /// Whether a checkbox is ticked.
    pub checked: bool,
    /// The options, for [`Widget::Select`] and [`Widget::ForeignKey`].
    pub choices: Vec<Choice>,
    /// Whether the field must be filled in.
    pub required: bool,
    /// Help text under the input.
    pub help: String,
    /// A character limit, when the schema states one.
    pub max_length: Option<u32>,
    /// What is wrong with it.
    pub errors: Vec<String>,
}

impl Field {
    /// Whether anything is wrong with this field.
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Whether the field is shown but not submitted.
    pub fn is_readonly(&self) -> bool {
        self.widget == Widget::ReadOnly
    }
}

/// What a browser posted: name/value pairs, as they arrived.
///
/// Built from `form_urlencoded`, from a multipart parser, or by hand in a
/// test. The admin does not parse a request body; it is handed the pairs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Submission {
    pairs: Vec<(String, String)>,
}

impl Submission {
    /// An empty submission.
    pub fn new() -> Self {
        Submission::default()
    }

    /// A submission from name/value pairs.
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Submission {
            pairs: pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    /// Adds a pair.
    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.pairs.push((name.into(), value.into()));
        self
    }

    /// The last value posted under a name.
    ///
    /// The last, because a checkbox is conventionally posted with a hidden
    /// `off` value in front of it so that unticking it still sends something.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Whether a name was posted at all, which is how a checkbox says "on".
    pub fn has(&self, name: &str) -> bool {
        self.pairs.iter().any(|(n, _)| n == name)
    }

    /// Every pair.
    pub fn pairs(&self) -> &[(String, String)] {
        &self.pairs
    }
}

/// A whole form: the fields, the record it edits, and what is wrong with it.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    /// The model key.
    pub model: String,
    /// The record being changed, or `None` on a create form.
    pub key: Option<PrimaryKey>,
    /// The fields, in column order.
    pub fields: Vec<Field>,
    /// Messages that belong to no one field.
    pub errors: Vec<String>,
}

impl Form {
    /// Builds a form for a model, optionally filled from a row.
    ///
    /// `choices` supplies the options for every [`Widget::ForeignKey`] field,
    /// keyed by column name — the admin fetches them from the target model and
    /// labels them with that model's configuration, which is why they arrive
    /// from outside rather than being looked up here.
    pub fn build(
        model: &ModelDescriptor,
        admin: &ModelAdmin,
        row: Option<&Row>,
        choices: &[(String, Vec<Choice>)],
    ) -> Form {
        let mut fields = Vec::new();
        for column in admin.form_for(model) {
            let relation = admin.relation_for(column);
            let mut widget = match admin.widget_for(&column.name) {
                Some(forced) => forced,
                None if relation.is_some() && column.relation.is_none() => Widget::ForeignKey,
                None => widget_for(column),
            };
            if admin.is_readonly(&column.name) && widget != Widget::Hidden {
                widget = Widget::ReadOnly;
            }
            if widget == Widget::Hidden {
                continue;
            }
            let stored = row.and_then(|r| r.get(&column.name));
            let options = choices
                .iter()
                .find(|(name, _)| *name == column.name)
                .map(|(_, c)| c.clone())
                .unwrap_or_else(|| {
                    column
                        .variants
                        .iter()
                        .map(|v| Choice::new(v.clone(), crate::model::humanise(v)))
                        .collect()
                });
            fields.push(Field {
                name: column.name.clone(),
                label: column.label(),
                widget,
                kind: column.kind,
                value: stored.map(Value::as_input).unwrap_or_default(),
                checked: stored.is_some_and(Value::as_bool),
                choices: options,
                required: !column.nullable && widget.is_editable() && widget != Widget::Checkbox,
                help: admin.help_for(&column.name).to_string(),
                max_length: column.max_length,
                errors: Vec::new(),
            });
        }
        Form {
            model: model.key.clone(),
            key: row.and_then(|r| PrimaryKey::from_row(model, r)),
            fields,
            errors: Vec::new(),
        }
    }

    /// Draws every editable field that `writable` refuses as read-only.
    ///
    /// The row policy's field mask, applied to the form: a locked field is
    /// shown and never rendered as an input, so the form does not send it,
    /// [`Form::accept`] does not take it, and [`Form::values`] does not
    /// write it. A request that sends one anyway was not made with this
    /// form, and the admin refuses it before this is called.
    pub fn lock(&mut self, writable: impl Fn(&str) -> bool) {
        for field in &mut self.fields {
            if field.widget.is_editable() && !writable(&field.name) {
                field.widget = Widget::ReadOnly;
                field.required = false;
            }
        }
    }

    /// Puts a submission into the fields.
    ///
    /// Every editable field takes its value from the submission, including the
    /// ones that are empty — an empty submitted field is a field the person
    /// cleared, and silently restoring the old value would be the same bug as
    /// losing what they typed, pointing the other way.
    pub fn accept(&mut self, submission: &Submission) {
        for field in &mut self.fields {
            if !field.widget.is_editable() {
                continue;
            }
            if field.widget == Widget::Checkbox {
                field.checked = submission
                    .get(&field.name)
                    .is_some_and(|v| matches!(v, "on" | "true" | "1" | "yes"));
                field.value = field.checked.to_string();
                continue;
            }
            if let Some(value) = submission.get(&field.name) {
                field.value = value.to_string();
            }
        }
    }

    /// Runs the checks the schema states, and reports whether the form passed.
    ///
    /// Only the three a column type can justify: present, numeric, short
    /// enough. Everything else is the model layer's — PLAN §7 puts validations
    /// inside the write path, so the admin learns the rest by trying and
    /// folds the answer back in with [`Form::reject`].
    pub fn validate(&mut self) -> bool {
        for field in &mut self.fields {
            if !field.widget.is_editable() {
                continue;
            }
            let trimmed = field.value.trim();
            if field.required && trimmed.is_empty() {
                field.errors.push("must be filled in".to_string());
                continue;
            }
            if trimmed.is_empty() {
                continue;
            }
            if field.kind.is_numeric() && trimmed.parse::<f64>().is_err() {
                field.errors.push("must be a number".to_string());
            }
            if field.kind == ColumnKind::Integer && trimmed.parse::<i64>().is_err() {
                field.errors.push("must be a whole number".to_string());
            }
            if let Some(max) = field.max_length {
                if trimmed.chars().count() > max as usize {
                    field
                        .errors
                        .push(format!("must be {max} characters or fewer"));
                }
            }
        }
        self.is_valid()
    }

    /// Folds errors the data source raised into the fields.
    ///
    /// The values are untouched, which is the entire point of this method
    /// existing rather than the caller rebuilding the form.
    pub fn reject(&mut self, errors: &FieldErrors) {
        for field in &mut self.fields {
            field
                .errors
                .extend(errors.on(&field.name).map(str::to_string));
        }
        self.errors.extend(errors.base().iter().cloned());
    }

    /// Adds a message that belongs to no one field.
    pub fn add_error(&mut self, message: impl Into<String>) {
        self.errors.push(message.into());
    }

    /// Whether nothing is wrong.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty() && self.fields.iter().all(|f| f.errors.is_empty())
    }

    /// One field by name.
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// The values to write, coerced by column type.
    ///
    /// Read-only and hidden fields are not included: a read-only field that
    /// round-trips through the browser is a read-only field a crafted post can
    /// change.
    pub fn values(&self) -> Vec<(String, Value)> {
        let mut out = Vec::with_capacity(self.fields.len());
        for field in &self.fields {
            if !field.widget.is_editable() {
                continue;
            }
            let trimmed = field.value.trim();
            let value = match field.widget {
                Widget::Checkbox => Value::Bool(field.checked),
                _ if trimmed.is_empty() && !field.required => Value::Null,
                _ => match field.kind {
                    ColumnKind::Integer => trimmed
                        .parse::<i64>()
                        .map(Value::Int)
                        .unwrap_or_else(|_| Value::Text(trimmed.to_string())),
                    ColumnKind::Float | ColumnKind::Decimal => trimmed
                        .parse::<f64>()
                        .map(Value::Float)
                        .unwrap_or_else(|_| Value::Text(trimmed.to_string())),
                    ColumnKind::Boolean => {
                        Value::Bool(matches!(trimmed, "true" | "1" | "on" | "yes" | "t"))
                    }
                    _ => Value::Text(trimmed.to_string()),
                },
            };
            out.push((field.name.clone(), value));
        }
        out
    }
}
