//! Writes the missing ids into model source text.
//!
//! This is the scaffolder half of PLAN.md §11: an id is written once, by a
//! tool, and never by hand. Each insertion is one new line above the struct
//! or the field, indented like it, so every other byte of the file stays as
//! it was. A model that has no attribute to remove the ids before SeaORM
//! sees them gets `#[brocanth_model::ids]` as its first attribute.
//!
//! A field that already has an id keeps it. That is the whole rule that makes
//! a rename a rename: the id stays on the field while the name changes.

use crate::scan::{scan_source, Model, Pos, Problem};

/// The result of [`stamp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamped {
    /// The file with the ids in it.
    pub text: String,
    /// One line per id written: `post`, `post.title`, ..
    pub written: Vec<String>,
}

/// Give every table and column in `text` an id. `new_id` makes each one.
pub fn stamp(
    file: &str,
    text: &str,
    mut new_id: impl FnMut() -> String,
) -> Result<Stamped, Vec<Problem>> {
    let models = scan_source(file, text)?;
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();
    // (line, order, text). `order` puts `ids` above the struct's own id when
    // the two go above the same line.
    let mut inserts: Vec<(usize, u8, String)> = Vec::new();
    let mut problems = Vec::new();
    let mut written = Vec::new();

    for m in &models {
        let mut place = |pos: Pos, order: u8, attr: String, what: &str| match own_line(&lines, pos)
        {
            Some(indent) => inserts.push((pos.line, order, format!("{indent}{attr}\n"))),
            None => problems.push(Problem {
                code: "shared-line",
                at: format!("{}:{}:{}", m.file, pos.line, pos.column + 1),
                message: format!(
                    "{what} does not start its own line. Put it on its own line, \
                         or write its id by hand."
                ),
            }),
        };
        if !m.strips_ids {
            place(
                m.first_attribute,
                0,
                "#[brocanth_model::ids]".to_owned(),
                &describe_model(m),
            );
        }
        if m.id.is_none() {
            place(m.id_goes, 1, id_attr(&new_id()), &describe_model(m));
            written.push(m.table.clone());
        }
        for f in &m.fields {
            if f.id.is_none() {
                let what = format!("Field `{}.{}`", m.table, f.def.name);
                place(f.id_goes, 1, id_attr(&new_id()), &what);
                written.push(format!("{}.{}", m.table, f.def.name));
            }
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }

    inserts.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    for (line, _, attr) in inserts {
        lines.insert(line - 1, attr);
    }
    Ok(Stamped {
        text: lines.concat(),
        written,
    })
}

fn describe_model(m: &Model) -> String {
    format!("Model `{}` (table `{}`)", m.ident, m.table)
}

fn id_attr(id: &str) -> String {
    format!("#[brocanth(id = \"{id}\")]")
}

/// The indentation of the line `pos` is on, when nothing precedes `pos` on it.
fn own_line(lines: &[String], pos: Pos) -> Option<String> {
    let line = lines.get(pos.line.checked_sub(1)?)?;
    let before: String = line.chars().take(pos.column).collect();
    before.chars().all(char::is_whitespace).then_some(before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::schema_of;

    const BARE: &str = r#"use brocanth_model::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "post")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub title: String,
}
"#;

    fn ids() -> impl FnMut() -> String {
        let mut n = 0;
        move || {
            n += 1;
            format!("01JBQ7R8V4K2M9X3T6N5P0WZ{n:02}")
        }
    }

    #[test]
    fn every_table_and_column_gets_an_id_and_the_rest_of_the_file_stays() {
        let stamped = stamp("post.rs", BARE, ids()).unwrap();
        assert_eq!(
            stamped.text,
            r#"use brocanth_model::prelude::*;

#[brocanth_model::ids]
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "post")]
#[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ01")]
pub struct Model {
    #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ02")]
    #[sea_orm(primary_key)]
    pub id: i32,
    #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ03")]
    pub title: String,
}
"#
        );
        assert_eq!(stamped.written, vec!["post", "post.id", "post.title"]);
        let models = scan_source("post.rs", &stamped.text).unwrap();
        assert!(schema_of(&models).is_ok());
    }

    #[test]
    fn a_second_run_writes_nothing() {
        let once = stamp("post.rs", BARE, ids()).unwrap();
        let twice = stamp("post.rs", &once.text, ids()).unwrap();
        assert_eq!(twice.text, once.text);
        assert!(twice.written.is_empty());
    }

    #[test]
    fn a_new_field_gets_a_new_id_and_the_old_ones_stay() {
        let once = stamp("post.rs", BARE, ids()).unwrap();
        let grown = once.text.replace(
            "    pub title: String,\n",
            "    pub title: String,\n    pub views: i32,\n",
        );
        let twice = stamp("post.rs", &grown, || "01JBQ7R8V4K2M9X3T6N5P0WZ99".into()).unwrap();
        assert_eq!(twice.written, vec!["post.views"]);
        assert!(twice.text.contains("WZ03\")]\n    pub title"));
        assert!(twice.text.contains("WZ99\")]\n    pub views"));
    }

    #[test]
    fn validates_already_removes_the_ids_so_ids_is_not_added() {
        let text = BARE.replace(
            "#[sea_orm::model]",
            "#[brocanth_model::validates(title(presence))]\n#[sea_orm::model]",
        );
        let stamped = stamp("post.rs", &text, ids()).unwrap();
        assert!(
            !stamped.text.contains("brocanth_model::ids"),
            "{}",
            stamped.text
        );
    }

    #[test]
    fn a_field_that_shares_its_line_is_refused() {
        let text = BARE.replace(
            "    pub id: i32,\n    pub title",
            "    pub id: i32, pub title",
        );
        let problems = stamp("post.rs", &text, ids()).unwrap_err();
        assert_eq!(
            problems[0].message,
            "Field `post.title` does not start its own line. Put it on its own line, \
             or write its id by hand."
        );
    }
}
