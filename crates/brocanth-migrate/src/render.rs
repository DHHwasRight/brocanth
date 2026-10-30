//! A list of changes as a SeaORM migration file.
//!
//! The file names tables and columns with `Alias::new("..")`, not with the
//! entity's `Column` enum. A migration must keep compiling after the model
//! changes again, and the enum describes only the latest model.
//!
//! The output is laid out the way `rustfmt` lays it out, so the first edit a
//! developer makes is not a reformat. The layout rules are the two widths
//! rustfmt uses by default: a chain or an argument list goes on one line when
//! it is at most 60 characters and the line is at most 100.

use crate::diff::Change;
use crate::schema::{Field, Table};

const MAX_WIDTH: usize = 100;
const SMALL_WIDTH: usize = 60;

/// A migration file. `name` is what `MigrationName::name` returns, which is
/// the file stem by SeaORM's convention: `m20261030_120000_rename_title`.
pub fn render(name: &str, changes: &[Change]) -> String {
    let mut out = String::from(
        "// Written by `brocanth migrate make`. Read it before you commit it.\n\
         //\n\
         // It is a proposal: edit it when it is wrong. migrations/schema.json\n\
         // records the models, not this file.\n\
         //\n",
    );
    for change in changes {
        out.push_str(&format!("// - {}\n", change.describe()));
    }
    out.push_str(&format!(
        "\n\
         use brocanth_model::sea_orm_migration::prelude::*;\n\
         \n\
         pub struct Migration;\n\
         \n\
         impl MigrationName for Migration {{\n\
         \x20   fn name(&self) -> &str {{\n\
         \x20       \"{name}\"\n\
         \x20   }}\n\
         }}\n\
         \n\
         #[async_trait::async_trait]\n\
         impl MigrationTrait for Migration {{\n"
    ));
    let down: Vec<Change> = changes.iter().rev().map(Change::inverse).collect();
    body(&mut out, "up", changes);
    out.push('\n');
    body(&mut out, "down", &down);
    out.push_str("}\n");
    out
}

fn body(out: &mut String, which: &str, changes: &[Change]) {
    out.push_str(&format!(
        "    async fn {which}(&self, manager: &SchemaManager) -> Result<(), DbErr> {{\n"
    ));
    let mut lines = Vec::new();
    for change in changes {
        lines.push(format!("// {}", change.describe()));
        if let Some(check) = check(change) {
            lines.push(format!("// Check: {check}"));
        }
        let (method, stmt) = statement(change);
        let call = E::chain(
            "manager",
            vec![Link::call(method, vec![stmt]), Link::field("await")],
        );
        layout(&call, 8, "?;", &mut lines);
    }
    for line in lines {
        if line.starts_with("//") {
            out.push_str("        ");
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("        Ok(())\n    }\n");
}

/// What a reader must check before the commit, when there is something.
fn check(change: &Change) -> Option<&'static str> {
    match change {
        Change::AddColumn { field, .. } if !field.nullable && !field.auto_increment => {
            Some("NOT NULL with no default fails on a table with rows. Add a default.")
        }
        Change::DropColumn { .. } | Change::DropTable(_) => {
            Some("this deletes data. The inverse step does not restore the data.")
        }
        Change::AlterColumn { .. } => {
            Some("SQLite cannot alter a column. On Postgres, check the data for the new type.")
        }
        _ => None,
    }
}

fn alias(name: &str) -> E {
    E::Atom(format!("Alias::new({name:?})"))
}

fn statement(change: &Change) -> (&'static str, E) {
    let alter = |table: &str, link: Link| {
        E::chain(
            "Table::alter()",
            vec![
                Link::call("table", vec![alias(table)]),
                link,
                Link::call("to_owned", vec![]),
            ],
        )
    };
    match change {
        Change::CreateTable(t) => ("create_table", create(t)),
        Change::DropTable(t) => (
            "drop_table",
            E::chain(
                "Table::drop()",
                vec![
                    Link::call("table", vec![alias(&t.name)]),
                    Link::call("to_owned", vec![]),
                ],
            ),
        ),
        Change::RenameTable { from, to } => (
            "rename_table",
            E::chain(
                "Table::rename()",
                vec![
                    Link::call("table", vec![alias(from), alias(to)]),
                    Link::call("to_owned", vec![]),
                ],
            ),
        ),
        Change::AddColumn { table, field } => (
            "alter_table",
            alter(table, Link::call("add_column", vec![column(field, true)])),
        ),
        Change::DropColumn { table, field } => (
            "alter_table",
            alter(table, Link::call("drop_column", vec![alias(&field.name)])),
        ),
        Change::RenameColumn { table, from, to } => (
            "alter_table",
            alter(
                table,
                Link::call("rename_column", vec![alias(from), alias(to)]),
            ),
        ),
        Change::AlterColumn {
            table,
            column: c,
            to,
            ..
        } => {
            let named = Field {
                name: c.clone(),
                ..to.clone()
            };
            (
                "alter_table",
                alter(
                    table,
                    Link::call("modify_column", vec![column(&named, true)]),
                ),
            )
        }
    }
}

fn create(t: &Table) -> E {
    let keys: Vec<&Field> = t.fields.iter().filter(|f| f.primary_key).collect();
    let composite = keys.len() > 1;
    let mut links = vec![Link::call("table", vec![alias(&t.name)])];
    for f in &t.fields {
        links.push(Link::call("col", vec![column(f, !composite)]));
    }
    if composite {
        let cols = keys
            .iter()
            .map(|f| Link::call("col", vec![alias(&f.name)]))
            .collect();
        links.push(Link::call(
            "primary_key",
            vec![E::chain("Index::create()", cols)],
        ));
    }
    links.push(Link::call("to_owned", vec![]));
    E::chain("Table::create()", links)
}

fn column(f: &Field, key_here: bool) -> E {
    let mut links = vec![Link::call(&f.kind, vec![])];
    links.push(Link::call(
        if f.nullable { "null" } else { "not_null" },
        vec![],
    ));
    if f.auto_increment {
        links.push(Link::call("auto_increment", vec![]));
    }
    if f.primary_key && key_here {
        links.push(Link::call("primary_key", vec![]));
    }
    if f.unique {
        links.push(Link::call("unique_key", vec![]));
    }
    E::chain(&format!("ColumnDef::new(Alias::new({:?}))", f.name), links)
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// An expression: a fixed piece of text, or a head with a method chain.
enum E {
    Atom(String),
    Chain(String, Vec<Link>),
}

/// `.name(args)`, or `.name` when `args` is `None`.
struct Link {
    name: String,
    args: Option<Vec<E>>,
}

impl Link {
    fn call(name: &str, args: Vec<E>) -> Self {
        Self {
            name: name.to_owned(),
            args: Some(args),
        }
    }

    fn field(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            args: None,
        }
    }

    fn flat(&self) -> String {
        match &self.args {
            None => format!(".{}", self.name),
            Some(args) => format!(".{}({})", self.name, flat_args(args)),
        }
    }
}

impl E {
    fn chain(head: &str, links: Vec<Link>) -> Self {
        E::Chain(head.to_owned(), links)
    }

    fn flat(&self) -> String {
        match self {
            E::Atom(text) => text.clone(),
            E::Chain(head, links) => {
                let mut s = head.clone();
                for link in links {
                    s.push_str(&link.flat());
                }
                s
            }
        }
    }
}

fn flat_args(args: &[E]) -> String {
    args.iter().map(E::flat).collect::<Vec<_>>().join(", ")
}

fn pad(indent: usize) -> String {
    " ".repeat(indent)
}

fn layout(e: &E, indent: usize, suffix: &str, out: &mut Vec<String>) {
    let one = e.flat();
    let fits = indent + one.len() + suffix.len() <= MAX_WIDTH;
    match e {
        E::Atom(_) => out.push(format!("{}{one}{suffix}", pad(indent))),
        E::Chain(head, links) => {
            if links.is_empty() || (one.len() <= SMALL_WIDTH && fits) {
                out.push(format!("{}{one}{suffix}", pad(indent)));
                return;
            }
            out.push(format!("{}{head}", pad(indent)));
            for (i, link) in links.iter().enumerate() {
                let last = i + 1 == links.len();
                layout_link(link, indent + 4, if last { suffix } else { "" }, out);
            }
        }
    }
}

fn layout_link(link: &Link, indent: usize, suffix: &str, out: &mut Vec<String>) {
    let Some(args) = &link.args else {
        out.push(format!("{}.{}{suffix}", pad(indent), link.name));
        return;
    };
    let inner = flat_args(args);
    let line = link.flat();
    if inner.len() <= SMALL_WIDTH && indent + line.len() + suffix.len() <= MAX_WIDTH {
        out.push(format!("{}{line}{suffix}", pad(indent)));
        return;
    }
    out.push(format!("{}.{}(", pad(indent), link.name));
    for arg in args {
        layout(arg, indent + 4, ",", out);
    }
    out.push(format!("{}){suffix}", pad(indent)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str, kind: &str) -> Field {
        Field {
            id: String::new(),
            name: name.into(),
            kind: kind.into(),
            nullable: false,
            primary_key: false,
            auto_increment: false,
            unique: false,
        }
    }

    #[test]
    fn a_rename_renders_as_rename_column_and_its_down_renames_back() {
        let text = render(
            "m20261030_120000_rename_title",
            &[Change::RenameColumn {
                table: "post".into(),
                from: "title".into(),
                to: "headline".into(),
            }],
        );
        assert!(
            text.contains(".rename_column(Alias::new(\"title\"), Alias::new(\"headline\"))"),
            "{text}"
        );
        assert!(
            text.contains(".rename_column(Alias::new(\"headline\"), Alias::new(\"title\"))"),
            "{text}"
        );
        assert!(!text.contains("drop_column"), "{text}");
        assert!(text.contains("\"m20261030_120000_rename_title\""), "{text}");
    }

    #[test]
    fn a_composite_key_is_declared_on_the_table() {
        let mut a = field("a", "integer");
        a.primary_key = true;
        let mut b = field("b", "integer");
        b.primary_key = true;
        let text = render(
            "m",
            &[Change::CreateTable(Table {
                id: String::new(),
                name: "pair".into(),
                fields: vec![a, b],
            })],
        );
        assert!(!text.contains(".primary_key()"), "{text}");
        assert!(
            text.contains(
                ".primary_key(Index::create().col(Alias::new(\"a\")).col(Alias::new(\"b\")))"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_long_chain_breaks_and_a_short_one_does_not() {
        let mut out = Vec::new();
        layout(&column(&field("x", "string"), true), 0, "", &mut out);
        assert_eq!(
            out,
            vec!["ColumnDef::new(Alias::new(\"x\")).string().not_null()"]
        );
        let mut long = field("a_rather_long_column_name", "string");
        long.unique = true;
        let mut out = Vec::new();
        layout(&column(&long, true), 0, "", &mut out);
        assert_eq!(out.len(), 4, "{out:#?}");
    }
}
