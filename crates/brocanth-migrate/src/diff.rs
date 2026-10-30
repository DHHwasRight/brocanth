//! Two snapshots in, a list of changes out. Tables and fields match by id, so
//! a new name on a known id is a rename and never a drop plus an add.

use std::collections::BTreeMap;

use crate::schema::{Field, Schema, Table};

/// One schema change. Every change knows its inverse, so the `down` of a
/// migration is the `up` read backwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A table the snapshot does not have.
    CreateTable(Table),
    /// A table the models no longer have. Carries the old definition, so the
    /// inverse can create it again.
    DropTable(Table),
    /// A known table id under a new name.
    RenameTable {
        /// The name before.
        from: String,
        /// The name after.
        to: String,
    },
    /// A field the snapshot does not have.
    AddColumn {
        /// The table, by its current name.
        table: String,
        /// The new column.
        field: Field,
    },
    /// A field the models no longer have.
    DropColumn {
        /// The table, by its current name.
        table: String,
        /// The old column, so the inverse can add it again.
        field: Field,
    },
    /// A known field id under a new name.
    RenameColumn {
        /// The table, by its current name.
        table: String,
        /// The name before.
        from: String,
        /// The name after.
        to: String,
    },
    /// A known field id with a new type, nullability, key or uniqueness.
    AlterColumn {
        /// The table, by its current name.
        table: String,
        /// The column, by its current name.
        column: String,
        /// The definition before.
        from: Field,
        /// The definition after.
        to: Field,
    },
}

impl Change {
    /// The change that undoes this one.
    pub fn inverse(&self) -> Change {
        match self.clone() {
            Change::CreateTable(t) => Change::DropTable(t),
            Change::DropTable(t) => Change::CreateTable(t),
            Change::RenameTable { from, to } => Change::RenameTable { from: to, to: from },
            Change::AddColumn { table, field } => Change::DropColumn { table, field },
            Change::DropColumn { table, field } => Change::AddColumn { table, field },
            Change::RenameColumn { table, from, to } => Change::RenameColumn {
                table,
                from: to,
                to: from,
            },
            Change::AlterColumn {
                table,
                column,
                from,
                to,
            } => Change::AlterColumn {
                table,
                column,
                from: to,
                to: from,
            },
        }
    }

    /// One line for a person.
    pub fn describe(&self) -> String {
        match self {
            Change::CreateTable(t) => format!("create table `{}`", t.name),
            Change::DropTable(t) => format!("drop table `{}`", t.name),
            Change::RenameTable { from, to } => format!("rename table `{from}` to `{to}`"),
            Change::AddColumn { table, field } => {
                format!("add column `{table}.{}` ({})", field.name, field.kind)
            }
            Change::DropColumn { table, field } => {
                format!("drop column `{table}.{}`", field.name)
            }
            Change::RenameColumn { table, from, to } => {
                format!("rename column `{table}.{from}` to `{to}`")
            }
            Change::AlterColumn {
                table,
                column,
                from,
                to,
            } => format!(
                "alter column `{table}.{column}` ({} to {})",
                spelled(from),
                spelled(to)
            ),
        }
    }

    /// `true` when the change can lose data.
    pub fn destructive(&self) -> bool {
        matches!(
            self,
            Change::DropTable(_) | Change::DropColumn { .. } | Change::AlterColumn { .. }
        )
    }
}

fn spelled(f: &Field) -> String {
    let mut s = f.kind.clone();
    s.push_str(if f.nullable { " null" } else { " not null" });
    if f.unique {
        s.push_str(" unique");
    }
    if f.primary_key {
        s.push_str(" primary key");
    }
    s
}

/// What it takes to go from `old` to `new`.
///
/// The order is the order the statements run in: per table, a rename first,
/// then column renames, alterations and additions, then column drops. New
/// tables come with their table, dropped tables come last. Every column change
/// names the table and the column by their new names, because by then the
/// renames have run.
pub fn diff(old: &Schema, new: &Schema) -> Vec<Change> {
    let before: BTreeMap<&str, &Table> = old.tables.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut changes = Vec::new();

    for table in &new.tables {
        let Some(was) = before.get(table.id.as_str()) else {
            changes.push(Change::CreateTable(table.clone()));
            continue;
        };
        if was.name != table.name {
            changes.push(Change::RenameTable {
                from: was.name.clone(),
                to: table.name.clone(),
            });
        }
        let fields: BTreeMap<&str, &Field> =
            was.fields.iter().map(|f| (f.id.as_str(), f)).collect();
        for field in &table.fields {
            match fields.get(field.id.as_str()) {
                None => changes.push(Change::AddColumn {
                    table: table.name.clone(),
                    field: field.clone(),
                }),
                Some(prior) => {
                    if prior.name != field.name {
                        changes.push(Change::RenameColumn {
                            table: table.name.clone(),
                            from: prior.name.clone(),
                            to: field.name.clone(),
                        });
                    }
                    if !prior.same_definition(field) {
                        changes.push(Change::AlterColumn {
                            table: table.name.clone(),
                            column: field.name.clone(),
                            from: Field {
                                name: field.name.clone(),
                                ..(*prior).clone()
                            },
                            to: field.clone(),
                        });
                    }
                }
            }
        }
        for prior in &was.fields {
            if !table.fields.iter().any(|f| f.id == prior.id) {
                changes.push(Change::DropColumn {
                    table: table.name.clone(),
                    field: prior.clone(),
                });
            }
        }
    }

    for table in &old.tables {
        if !new.tables.iter().any(|t| t.id == table.id) {
            changes.push(Change::DropTable(table.clone()));
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(id: &str, name: &str, kind: &str) -> Field {
        Field {
            id: id.into(),
            name: name.into(),
            kind: kind.into(),
            nullable: false,
            primary_key: false,
            auto_increment: false,
            unique: false,
        }
    }

    fn table(id: &str, name: &str, fields: Vec<Field>) -> Table {
        Table {
            id: id.into(),
            name: name.into(),
            fields,
        }
    }

    fn post(fields: Vec<Field>) -> Schema {
        Schema::new(vec![table("T1", "post", fields)])
    }

    #[test]
    fn a_rename_by_id_is_a_rename_and_not_a_drop_and_an_add() {
        let old = post(vec![field("F1", "title", "string")]);
        let new = post(vec![field("F1", "headline", "string")]);
        assert_eq!(
            diff(&old, &new),
            vec![Change::RenameColumn {
                table: "post".into(),
                from: "title".into(),
                to: "headline".into(),
            }]
        );
    }

    #[test]
    fn the_same_rename_with_a_new_id_is_a_drop_and_an_add() {
        // The case the ids exist for, from the other side: a new id means a
        // new field, whatever the name says.
        let old = post(vec![field("F1", "title", "string")]);
        let new = post(vec![field("F2", "headline", "string")]);
        let changes = diff(&old, &new);
        assert!(
            matches!(changes[0], Change::AddColumn { .. }),
            "{changes:?}"
        );
        assert!(
            matches!(changes[1], Change::DropColumn { .. }),
            "{changes:?}"
        );
    }

    #[test]
    fn an_added_field_is_an_add() {
        let old = post(vec![field("F1", "title", "string")]);
        let new = post(vec![
            field("F1", "title", "string"),
            field("F2", "views", "integer"),
        ]);
        assert_eq!(
            diff(&old, &new),
            vec![Change::AddColumn {
                table: "post".into(),
                field: field("F2", "views", "integer"),
            }]
        );
    }

    #[test]
    fn a_removed_field_is_a_drop_that_remembers_its_definition() {
        let old = post(vec![
            field("F1", "title", "string"),
            field("F2", "views", "integer"),
        ]);
        let new = post(vec![field("F1", "title", "string")]);
        let changes = diff(&old, &new);
        assert_eq!(
            changes,
            vec![Change::DropColumn {
                table: "post".into(),
                field: field("F2", "views", "integer"),
            }]
        );
        assert!(changes[0].destructive());
        assert_eq!(
            changes[0].inverse(),
            Change::AddColumn {
                table: "post".into(),
                field: field("F2", "views", "integer"),
            }
        );
    }

    #[test]
    fn a_type_change_is_an_alter() {
        let old = post(vec![field("F1", "views", "integer")]);
        let mut wider = field("F1", "views", "big_integer");
        wider.nullable = true;
        let new = post(vec![wider.clone()]);
        let changes = diff(&old, &new);
        assert_eq!(
            changes,
            vec![Change::AlterColumn {
                table: "post".into(),
                column: "views".into(),
                from: field("F1", "views", "integer"),
                to: wider,
            }]
        );
        assert_eq!(
            changes[0].describe(),
            "alter column `post.views` (integer not null to big_integer null)"
        );
    }

    #[test]
    fn a_rename_and_a_type_change_on_one_field_rename_first() {
        let old = post(vec![field("F1", "views", "integer")]);
        let new = post(vec![field("F1", "view_count", "big_integer")]);
        let changes = diff(&old, &new);
        assert!(matches!(&changes[0], Change::RenameColumn { to, .. } if to == "view_count"));
        assert!(
            matches!(&changes[1], Change::AlterColumn { column, from, .. }
                if column == "view_count" && from.kind == "integer"),
            "the alter names the column by its new name: {changes:?}"
        );
    }

    #[test]
    fn a_table_rename_by_id_is_a_rename() {
        let old = post(vec![field("F1", "title", "string")]);
        let new = Schema::new(vec![table(
            "T1",
            "article",
            vec![field("F1", "title", "string")],
        )]);
        assert_eq!(
            diff(&old, &new),
            vec![Change::RenameTable {
                from: "post".into(),
                to: "article".into(),
            }]
        );
    }

    #[test]
    fn a_new_table_is_created_and_an_old_one_dropped() {
        let old = post(vec![field("F1", "title", "string")]);
        let new = Schema::new(vec![table(
            "T2",
            "user",
            vec![field("F9", "name", "string")],
        )]);
        let changes = diff(&old, &new);
        assert!(matches!(&changes[0], Change::CreateTable(t) if t.name == "user"));
        assert!(matches!(&changes[1], Change::DropTable(t) if t.name == "post"));
    }

    #[test]
    fn no_change_is_no_change() {
        let schema = post(vec![field("F1", "title", "string")]);
        assert!(diff(&schema, &schema).is_empty());
        assert!(diff(&Schema::default(), &Schema::default()).is_empty());
    }

    #[test]
    fn every_inverse_inverts_back() {
        let old = post(vec![
            field("F1", "views", "integer"),
            field("F3", "gone", "string"),
        ]);
        let new = Schema::new(vec![
            table(
                "T1",
                "article",
                vec![field("F1", "view_count", "big_integer")],
            ),
            table("T2", "user", vec![field("F9", "name", "string")]),
        ]);
        for change in diff(&old, &new) {
            assert_eq!(change.inverse().inverse(), change);
        }
    }
}
