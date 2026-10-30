//! Reads model source text: which structs are tables, which fields are
//! columns, and what id each one carries.
//!
//! A source scan and not the entity registry, for two reasons. The id lives
//! in the text, so the text is the authority on it. And the CLI writes a
//! migration without a build of the project, which can fail for reasons that
//! have nothing to do with the schema.
//!
//! A struct is a table when it carries `#[sea_orm(table_name = "..")]`. A field
//! is a column unless it is a relation (`has_many`, `has_one`, `belongs_to`, or
//! a `HasMany`/`HasOne`/`BelongsTo` type) or `#[sea_orm(ignore)]`.

use std::collections::BTreeMap;

use proc_macro2::Span;
use quote::ToTokens;
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, GenericArgument, Item, ItemStruct, Lit, Meta, PathArguments, Token, Type,
};

use crate::id::is_id;
use crate::schema::{Field, Schema, Table};

/// Something in the source that stops the scan. `at` is `file:line:col`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// A short name for the rule: `no-id`, `duplicate-id`, `no-column-type`, ..
    pub code: &'static str,
    /// `file:line:col`.
    pub at: String,
    /// One sentence or more, in strict STE.
    pub message: String,
}

/// A position: 1-based line, 0-based character column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pos {
    /// 1-based.
    pub line: usize,
    /// 0-based, in characters.
    pub column: usize,
}

impl Pos {
    fn of(span: Span) -> Self {
        let lc = span.start();
        Self {
            line: lc.line,
            column: lc.column,
        }
    }
}

/// One struct that is a table.
#[derive(Debug, Clone)]
pub struct Model {
    /// The file label given to [`scan_source`].
    pub file: String,
    /// The Rust name of the struct, usually `Model`.
    pub ident: String,
    /// The table name.
    pub table: String,
    /// The id, when the struct carries one.
    pub id: Option<String>,
    /// The struct name, for messages.
    pub at: Pos,
    /// Where the struct's id goes: above `pub struct`, below the derives.
    pub id_goes: Pos,
    /// The first attribute of the struct, where `#[brocanth_model::ids]` goes.
    pub first_attribute: Pos,
    /// `true` when `#[brocanth_model::ids]` or `#[brocanth_model::validates]`
    /// is on the struct, so the id attributes compile.
    pub strips_ids: bool,
    /// The columns, in declaration order.
    pub fields: Vec<ScannedField>,
}

/// One column field.
#[derive(Debug, Clone)]
pub struct ScannedField {
    /// The id, when the field carries one.
    pub id: Option<String>,
    /// Everything but the id.
    pub def: Field,
    /// The field name, for messages.
    pub at: Pos,
    /// The first token of the field, attributes included: where its id goes.
    pub id_goes: Pos,
}

impl Model {
    /// `file:line:col` of the struct name.
    pub fn location(&self) -> String {
        format!("{}:{}:{}", self.file, self.at.line, self.at.column + 1)
    }
}

impl ScannedField {
    /// `file:line:col` of the field name, in `model`'s file.
    pub fn location(&self, model: &Model) -> String {
        format!("{}:{}:{}", model.file, self.at.line, self.at.column + 1)
    }
}

/// Every table in one file. `file` is the label messages use.
pub fn scan_source(file: &str, text: &str) -> Result<Vec<Model>, Vec<Problem>> {
    let parsed = syn::parse_file(text).map_err(|e| {
        let at = Pos::of(e.span());
        vec![Problem {
            code: "parse",
            at: format!("{file}:{}:{}", at.line, at.column + 1),
            message: format!("This file is not valid Rust: {e}."),
        }]
    })?;
    let mut models = Vec::new();
    let mut problems = Vec::new();
    walk(file, &parsed.items, &mut models, &mut problems);
    if problems.is_empty() {
        Ok(models)
    } else {
        Err(problems)
    }
}

fn walk(file: &str, items: &[Item], models: &mut Vec<Model>, problems: &mut Vec<Problem>) {
    for item in items {
        match item {
            Item::Struct(s) => {
                if let Some(model) = model(file, s, problems) {
                    models.push(model);
                }
            }
            Item::Mod(m) => {
                if let Some((_, items)) = &m.content {
                    walk(file, items, models, problems);
                }
            }
            _ => {}
        }
    }
}

fn model(file: &str, s: &ItemStruct, problems: &mut Vec<Problem>) -> Option<Model> {
    let at = |span: Span| {
        let p = Pos::of(span);
        format!("{file}:{}:{}", p.line, p.column + 1)
    };
    let mut table = None;
    for attr in s.attrs.iter().filter(|a| a.path().is_ident("sea_orm")) {
        for (key, value) in pairs(attr, file, problems) {
            if key == "table_name" {
                table = value.and_then(|v| v.text);
            }
        }
    }
    let table = table?;

    let id = brocanth_id(&s.attrs, file, problems);
    let strips_ids = s.attrs.iter().any(|a| {
        let segments: Vec<String> = a
            .path()
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        let last = segments.last().map(String::as_str);
        matches!(last, Some("ids" | "validates"))
            && (segments.len() == 1 || segments[0] == "brocanth_model")
    });
    let start = start_of(&s.vis, s.struct_token.span);
    let first_attribute = s
        .attrs
        .first()
        .map_or(start, |a| Pos::of(a.pound_token.span));

    let mut fields = Vec::new();
    for f in &s.fields {
        let Some(ident) = &f.ident else { continue };
        let name_span = ident.span();
        let mut column = ident.to_string();
        let mut relation = false;
        let mut nullable = false;
        let mut primary_key = false;
        let mut auto_increment = None;
        let mut unique = false;
        let mut column_type = None;
        for attr in f.attrs.iter().filter(|a| a.path().is_ident("sea_orm")) {
            for (key, value) in pairs(attr, file, problems) {
                match key.as_str() {
                    "has_many" | "has_one" | "belongs_to" | "ignore" => relation = true,
                    "primary_key" => primary_key = true,
                    "unique" => unique = true,
                    "nullable" => nullable = true,
                    "auto_increment" => auto_increment = value.and_then(|v| v.flag),
                    "column_name" => {
                        if let Some(v) = value.and_then(|v| v.text) {
                            column = v;
                        }
                    }
                    "column_type" => column_type = value.and_then(|v| v.text),
                    _ => {}
                }
            }
        }
        let (inner, optional) = match option_inner(&f.ty) {
            Some(inner) => (inner, true),
            None => (&f.ty, false),
        };
        let key = type_key(inner).unwrap_or_else(|| compact(&inner.to_token_stream().to_string()));
        if relation
            || ["HasMany", "HasOne", "BelongsTo"]
                .iter()
                .any(|r| key.starts_with(r))
        {
            continue;
        }
        let kind = match &column_type {
            Some(ct) => kind_of_column_type(ct).ok_or_else(|| {
                format!(
                    "Field `{table}.{column}` has column type `{ct}`. No `ColumnDef` method \
                     matches this column type. Write this migration by hand."
                )
            }),
            None => kind_of_type(&key).ok_or_else(|| {
                format!(
                    "Field `{table}.{column}` has type `{key}`. No column type matches this \
                     type. Add `#[sea_orm(column_type = \"..\")]`, or write this migration \
                     by hand."
                )
            }),
        };
        let kind = match kind {
            Ok(kind) => kind,
            Err(message) => {
                problems.push(Problem {
                    code: "no-column-type",
                    at: at(name_span),
                    message,
                });
                continue;
            }
        };
        let integer = matches!(
            kind,
            "tiny_integer"
                | "small_integer"
                | "integer"
                | "big_integer"
                | "tiny_unsigned"
                | "small_unsigned"
                | "unsigned"
                | "big_unsigned"
        );
        let id_goes = f.attrs.first().map_or_else(
            || start_of(&f.vis, name_span),
            |a| Pos::of(a.pound_token.span),
        );
        fields.push(ScannedField {
            id: brocanth_id(&f.attrs, file, problems),
            def: Field {
                id: String::new(),
                name: column,
                kind: kind.to_owned(),
                nullable: nullable || optional,
                primary_key,
                auto_increment: primary_key && auto_increment.unwrap_or(integer),
                unique,
            },
            at: Pos::of(name_span),
            id_goes,
        });
    }

    Some(Model {
        file: file.to_owned(),
        ident: s.ident.to_string(),
        table,
        id,
        at: Pos::of(s.ident.span()),
        id_goes: start,
        first_attribute,
        strips_ids,
        fields,
    })
}

/// The first token of an item after its attributes.
fn start_of(vis: &syn::Visibility, otherwise: Span) -> Pos {
    match vis {
        syn::Visibility::Public(p) => Pos::of(p.span),
        syn::Visibility::Restricted(r) => Pos::of(r.pub_token.span),
        syn::Visibility::Inherited => Pos::of(otherwise),
    }
}

/// A value in `key = value`.
#[derive(Clone)]
struct Value {
    text: Option<String>,
    flag: Option<bool>,
}

/// The `key` and `key = value` entries of one `#[name(..)]` attribute.
fn pairs(
    attr: &Attribute,
    file: &str,
    problems: &mut Vec<Problem>,
) -> Vec<(String, Option<Value>)> {
    let metas = match attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) {
        Ok(m) => m,
        Err(e) => {
            let p = Pos::of(e.span());
            problems.push(Problem {
                code: "attribute",
                at: format!("{file}:{}:{}", p.line, p.column + 1),
                message: format!("This attribute cannot be read: {e}."),
            });
            return Vec::new();
        }
    };
    metas
        .iter()
        .map(|meta| {
            let key = meta
                .path()
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            let value = match meta {
                Meta::NameValue(nv) => Some(match &nv.value {
                    Expr::Lit(l) => match &l.lit {
                        Lit::Str(s) => Value {
                            text: Some(s.value()),
                            flag: None,
                        },
                        Lit::Bool(b) => Value {
                            text: None,
                            flag: Some(b.value),
                        },
                        _ => Value {
                            text: None,
                            flag: None,
                        },
                    },
                    _ => Value {
                        text: None,
                        flag: None,
                    },
                }),
                _ => None,
            };
            (key, value)
        })
        .collect()
}

/// The id in `#[brocanth(id = "..")]`, checked.
fn brocanth_id(attrs: &[Attribute], file: &str, problems: &mut Vec<Problem>) -> Option<String> {
    let mut found = None;
    for attr in attrs.iter().filter(|a| a.path().is_ident("brocanth")) {
        let p = Pos::of(attr.pound_token.span);
        let at = format!("{file}:{}:{}", p.line, p.column + 1);
        for (key, value) in pairs(attr, file, problems) {
            match (key.as_str(), value.and_then(|v| v.text)) {
                ("id", Some(id)) if is_id(&id) => found = Some(id),
                ("id", Some(id)) => problems.push(Problem {
                    code: "bad-id",
                    at: at.clone(),
                    message: format!(
                        "The id `{id}` is not a ULID. An id has 26 characters from \
                         `0123456789ABCDEFGHJKMNPQRSTVWXYZ`."
                    ),
                }),
                _ => problems.push(Problem {
                    code: "attribute",
                    at: at.clone(),
                    message: format!(
                        "`#[brocanth(..)]` has the key `{key}`. The only key is `id = \"..\"`."
                    ),
                }),
            }
        }
    }
    found
}

/// `T` in `Option<T>`.
fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(p) = ty else { return None };
    let last = p.path.segments.last()?;
    if last.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    match args.args.first()? {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    }
}

/// `chrono::DateTime<chrono::Utc>` as `DateTime<Utc>`: last segments only.
fn type_key(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) => {
            let seg = p.path.segments.last()?;
            let mut key = seg.ident.to_string();
            if let PathArguments::AngleBracketed(args) = &seg.arguments {
                let inner: Vec<String> = args
                    .args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArgument::Type(t) => type_key(t),
                        _ => None,
                    })
                    .collect();
                key.push('<');
                key.push_str(&inner.join(","));
                key.push('>');
            }
            Some(key)
        }
        Type::Group(g) => type_key(&g.elem),
        Type::Paren(p) => type_key(&p.elem),
        _ => None,
    }
}

fn compact(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The `ColumnDef` method SeaORM's own mapping gives a Rust type.
pub fn kind_of_type(key: &str) -> Option<&'static str> {
    Some(match key {
        "String" => "string",
        "i8" => "tiny_integer",
        "i16" => "small_integer",
        "i32" => "integer",
        "i64" => "big_integer",
        "u8" => "tiny_unsigned",
        "u16" => "small_unsigned",
        "u32" => "unsigned",
        "u64" => "big_unsigned",
        "f32" => "float",
        "f64" => "double",
        "bool" => "boolean",
        "Vec<u8>" => "blob",
        "NaiveDate" | "Date" | "ChronoDate" => "date",
        "NaiveTime" | "Time" | "ChronoTime" => "time",
        "NaiveDateTime" | "DateTime" | "ChronoDateTime" => "date_time",
        "DateTimeUtc"
        | "DateTimeLocal"
        | "DateTimeWithTimeZone"
        | "ChronoDateTimeUtc"
        | "ChronoDateTimeLocal"
        | "ChronoDateTimeWithTimeZone"
        | "DateTime<Utc>"
        | "DateTime<Local>"
        | "DateTime<FixedOffset>" => "timestamp_with_time_zone",
        "Uuid" => "uuid",
        "Json" | "Value" => "json_binary",
        "Decimal" => "decimal",
        _ => return None,
    })
}

/// The `ColumnDef` method for `#[sea_orm(column_type = "..")]`.
pub fn kind_of_column_type(column_type: &str) -> Option<&'static str> {
    let head = column_type.split('(').next().unwrap_or("").trim();
    Some(match head {
        "Char" => "char",
        "String" => "string",
        "Text" => "text",
        "TinyInteger" => "tiny_integer",
        "SmallInteger" => "small_integer",
        "Integer" => "integer",
        "BigInteger" => "big_integer",
        "TinyUnsigned" => "tiny_unsigned",
        "SmallUnsigned" => "small_unsigned",
        "Unsigned" => "unsigned",
        "BigUnsigned" => "big_unsigned",
        "Float" => "float",
        "Double" => "double",
        "Decimal" => "decimal",
        "DateTime" => "date_time",
        "Timestamp" => "timestamp",
        "TimestampWithTimeZone" => "timestamp_with_time_zone",
        "Time" => "time",
        "Date" => "date",
        "Binary" | "VarBinary" => "binary",
        "Blob" => "blob",
        "Boolean" => "boolean",
        "Money" => "money",
        "Json" => "json",
        "JsonBinary" => "json_binary",
        "Uuid" => "uuid",
        _ => return None,
    })
}

/// Every table, or the reasons there is no schema: a missing id or an id
/// used two times. Never a guess.
pub fn schema_of(models: &[Model]) -> Result<Schema, Vec<Problem>> {
    let mut problems = Vec::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    let mut tables: BTreeMap<&str, &Model> = BTreeMap::new();
    let mut out = Vec::new();
    for m in models {
        if let Some(other) = tables.insert(&m.table, m) {
            problems.push(Problem {
                code: "duplicate-table",
                at: m.location(),
                message: format!(
                    "The table `{}` is also declared at {}. Each table must be declared one time.",
                    m.table,
                    other.location()
                ),
            });
        }
        match &m.id {
            Some(id) => claim(&mut seen, id, m.location(), &mut problems),
            None => problems.push(Problem {
                code: "no-id",
                at: m.location(),
                message: format!(
                    "Model `{}` (table `{}`) has no id. Run `brocanth migrate ids`. \
                     Then run this command again.",
                    m.ident, m.table
                ),
            }),
        }
        let mut fields = Vec::new();
        for f in &m.fields {
            match &f.id {
                Some(id) => {
                    claim(&mut seen, id, f.location(m), &mut problems);
                    fields.push(Field {
                        id: id.clone(),
                        ..f.def.clone()
                    });
                }
                None => problems.push(Problem {
                    code: "no-id",
                    at: f.location(m),
                    message: format!(
                        "Field `{}.{}` has no id. Run `brocanth migrate ids`. \
                         Then run this command again.",
                        m.table, f.def.name
                    ),
                }),
            }
        }
        if let Some(id) = &m.id {
            out.push(Table {
                id: id.clone(),
                name: m.table.clone(),
                fields,
            });
        }
    }
    if problems.is_empty() {
        Ok(Schema::new(out))
    } else {
        Err(problems)
    }
}

/// Record that `id` is at `at`, or report that it was already seen.
fn claim(seen: &mut BTreeMap<String, String>, id: &str, at: String, problems: &mut Vec<Problem>) {
    if let Some(first) = seen.get(id) {
        problems.push(Problem {
            code: "duplicate-id",
            at,
            message: format!(
                "The id `{id}` is also at {first}. Each id must occur one time. \
                 Remove this attribute and run `brocanth migrate ids`."
            ),
        });
        return;
    }
    seen.insert(id.to_owned(), at);
}

#[cfg(test)]
mod tests {
    use super::*;

    const POST: &str = r#"
pub mod post {
    use brocanth_model::prelude::*;

    #[brocanth_model::ids]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "post")]
    #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1A")]
    pub struct Model {
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1B")]
        #[sea_orm(primary_key)]
        pub id: i32,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1C")]
        #[sea_orm(unique, column_name = "headline")]
        pub title: String,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1D")]
        pub body: Option<String>,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1E")]
        pub published_at: Option<chrono::DateTime<chrono::Utc>>,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1F")]
        #[sea_orm(column_type = "Text")]
        pub notes: String,
        #[sea_orm(belongs_to, from = "user_id", to = "id")]
        pub author: BelongsTo<super::user::Entity>,
    }
}

pub struct NotATable {
    pub x: i32,
}
"#;

    #[test]
    fn a_model_reads_as_a_table_with_its_ids() {
        let models = scan_source("post.rs", POST).unwrap();
        assert_eq!(
            models.len(),
            1,
            "a struct without table_name is not a table"
        );
        let schema = schema_of(&models).unwrap();
        let t = &schema.tables[0];
        assert_eq!(t.name, "post");
        assert_eq!(t.id, "01JBQ7R8V4K2M9X3T6N5P0WZ1A");
        let shape: Vec<(&str, &str, bool)> = t
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.kind.as_str(), f.nullable))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("id", "integer", false),
                ("headline", "string", false),
                ("body", "string", true),
                ("published_at", "timestamp_with_time_zone", true),
                ("notes", "text", false),
            ],
            "the relation is not a column"
        );
        assert!(t.fields[0].primary_key && t.fields[0].auto_increment);
        assert!(t.fields[1].unique);
        assert!(models[0].strips_ids);
    }

    #[test]
    fn a_missing_id_is_refused_with_where_it_is() {
        let text = POST.replace(
            "        #[brocanth(id = \"01JBQ7R8V4K2M9X3T6N5P0WZ1D\")]\n",
            "",
        );
        let problems = schema_of(&scan_source("post.rs", &text).unwrap()).unwrap_err();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].at, "post.rs:17:13");
        assert_eq!(
            problems[0].message,
            "Field `post.body` has no id. Run `brocanth migrate ids`. Then run this command again."
        );
    }

    #[test]
    fn a_copied_id_is_refused() {
        let text = POST.replace("WZ1D", "WZ1C");
        let problems = schema_of(&scan_source("post.rs", &text).unwrap()).unwrap_err();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].message.contains("is also at post.rs:"),
            "{problems:?}"
        );
    }

    #[test]
    fn a_malformed_id_is_refused() {
        let text = POST.replace("01JBQ7R8V4K2M9X3T6N5P0WZ1D", "abc");
        let problems = scan_source("post.rs", &text).unwrap_err();
        assert!(problems[0]
            .message
            .starts_with("The id `abc` is not a ULID."));
    }

    #[test]
    fn a_type_with_no_column_is_refused_rather_than_guessed() {
        let text = POST.replace("pub body: Option<String>", "pub body: Vec<String>");
        let problems = scan_source("post.rs", &text).unwrap_err();
        assert_eq!(
            problems[0].message,
            "Field `post.body` has type `Vec<String>`. No column type matches this type. \
             Add `#[sea_orm(column_type = \"..\")]`, or write this migration by hand."
        );
    }

    #[test]
    fn auto_increment_false_is_honoured() {
        let text = POST.replace(
            "#[sea_orm(primary_key)]",
            "#[sea_orm(primary_key, auto_increment = false)]",
        );
        let schema = schema_of(&scan_source("post.rs", &text).unwrap()).unwrap();
        assert!(!schema.tables[0].fields[0].auto_increment);
    }
}
