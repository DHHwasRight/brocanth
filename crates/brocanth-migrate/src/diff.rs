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
/// The order is the order the statements run in, in phases, so a name one
/// change frees is free before another change takes it:
///
/// 1. dropped tables;
/// 2. table renames;
/// 3. per table, dropped columns;
/// 4. per table, column renames;
/// 5. per table, alterations;
/// 6. per table, added columns;
/// 7. new tables.
///
/// A field replaced by a new field of the same name is a drop, then an add.
/// A rename onto a name another rename frees goes through a temporary name
/// first (`brocanth_tmp_<id>`), so a swap of two names works. Every column
/// change after phase 2 names its table by the new name, and every change
/// after phase 4 names its column by the new name.
pub fn diff(old: &Schema, new: &Schema) -> Vec<Change> {
    let before: BTreeMap<&str, &Table> = old.tables.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut changes = Vec::new();

    // 1. Dropped tables.
    for table in &old.tables {
        if !new.tables.iter().any(|t| t.id == table.id) {
            changes.push(Change::DropTable(table.clone()));
        }
    }

    // 2. Table renames.
    let renamed: Vec<(&str, &str, &str)> = new
        .tables
        .iter()
        .filter_map(|t| {
            let was = before.get(t.id.as_str())?;
            (was.name != t.name).then_some((t.id.as_str(), was.name.as_str(), t.name.as_str()))
        })
        .collect();
    for (from, to) in renames(&renamed) {
        changes.push(Change::RenameTable { from, to });
    }

    // Kept tables, by their new names from here on.
    let kept: Vec<(&Table, &Table)> = new
        .tables
        .iter()
        .filter_map(|t| before.get(t.id.as_str()).map(|was| (*was, t)))
        .collect();

    // 3. Dropped columns.
    for (was, table) in &kept {
        for prior in &was.fields {
            if !table.fields.iter().any(|f| f.id == prior.id) {
                changes.push(Change::DropColumn {
                    table: table.name.clone(),
                    field: prior.clone(),
                });
            }
        }
    }

    // 4. Column renames.
    for (was, table) in &kept {
        let fields: BTreeMap<&str, &Field> =
            was.fields.iter().map(|f| (f.id.as_str(), f)).collect();
        let renamed: Vec<(&str, &str, &str)> = table
            .fields
            .iter()
            .filter_map(|f| {
                let prior = fields.get(f.id.as_str())?;
                (prior.name != f.name).then_some((
                    f.id.as_str(),
                    prior.name.as_str(),
                    f.name.as_str(),
                ))
            })
            .collect();
        for (from, to) in renames(&renamed) {
            changes.push(Change::RenameColumn {
                table: table.name.clone(),
                from,
                to,
            });
        }
    }

    // 5. Alterations.
    for (was, table) in &kept {
        for field in &table.fields {
            let Some(prior) = was.fields.iter().find(|f| f.id == field.id) else {
                continue;
            };
            if !prior.same_definition(field) {
                changes.push(Change::AlterColumn {
                    table: table.name.clone(),
                    column: field.name.clone(),
                    from: Field {
                        name: field.name.clone(),
                        ..prior.clone()
                    },
                    to: field.clone(),
                });
            }
        }
    }

    // 6. Added columns.
    for (was, table) in &kept {
        for field in &table.fields {
            if !was.fields.iter().any(|f| f.id == field.id) {
                changes.push(Change::AddColumn {
                    table: table.name.clone(),
                    field: field.clone(),
                });
            }
        }
    }

    // 7. New tables.
    for table in &new.tables {
        if !before.contains_key(table.id.as_str()) {
            changes.push(Change::CreateTable(table.clone()));
        }
    }
    changes
}

/// The `(from, to)` steps for a set of renames `(id, from, to)`.
///
/// A rename whose old name is the new name of another rename moves to a
/// temporary name first. After that, no new name is still taken, so each
/// rename runs in the order given.
fn renames(renamed: &[(&str, &str, &str)]) -> Vec<(String, String)> {
    let targets: Vec<&str> = renamed.iter().map(|(_, _, to)| *to).collect();
    let mut steps = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for (id, from, _) in renamed {
        if targets.contains(from) {
            let temporary = format!("brocanth_tmp_{}", id.to_ascii_lowercase());
            steps.push((from.to_string(), temporary.clone()));
            current.push(temporary);
        } else {
            current.push(from.to_string());
        }
    }
    for (now, (_, _, to)) in current.into_iter().zip(renamed) {
        steps.push((now, to.to_string()));
    }
    steps
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
            matches!(changes[0], Change::DropColumn { .. }),
            "{changes:?}"
        );
        assert!(
            matches!(changes[1], Change::AddColumn { .. }),
            "{changes:?}"
        );
    }

    #[test]
    fn a_field_replaced_under_its_own_name_is_dropped_before_it_is_added() {
        let old = post(vec![field("F1", "title", "string")]);
        let new = post(vec![field("F2", "title", "text")]);
        assert_eq!(
            diff(&old, &new),
            vec![
                Change::DropColumn {
                    table: "post".into(),
                    field: field("F1", "title", "string"),
                },
                Change::AddColumn {
                    table: "post".into(),
                    field: field("F2", "title", "text"),
                },
            ]
        );
    }

    #[test]
    fn a_table_created_again_under_its_name_is_dropped_first() {
        let old = post(vec![field("F1", "title", "string")]);
        let new = Schema::new(vec![table(
            "T9",
            "post",
            vec![field("F2", "title", "string")],
        )]);
        let changes = diff(&old, &new);
        assert!(
            matches!(&changes[0], Change::DropTable(t) if t.id == "T1"),
            "{changes:?}"
        );
        assert!(
            matches!(&changes[1], Change::CreateTable(t) if t.id == "T9"),
            "{changes:?}"
        );
    }

    #[test]
    fn a_rename_onto_a_dropped_name_runs_after_the_drop() {
        let old = post(vec![
            field("F1", "title", "string"),
            field("F2", "name", "string"),
        ]);
        let new = post(vec![field("F2", "title", "string")]);
        assert_eq!(
            diff(&old, &new),
            vec![
                Change::DropColumn {
                    table: "post".into(),
                    field: field("F1", "title", "string"),
                },
                Change::RenameColumn {
                    table: "post".into(),
                    from: "name".into(),
                    to: "title".into(),
                },
            ]
        );
    }

    #[test]
    fn a_swap_of_two_names_goes_through_a_temporary_name() {
        let old = post(vec![field("F1", "a", "string"), field("F2", "b", "string")]);
        let new = post(vec![field("F1", "b", "string"), field("F2", "a", "string")]);
        let rename = |from: &str, to: &str| Change::RenameColumn {
            table: "post".into(),
            from: from.into(),
            to: to.into(),
        };
        assert_eq!(
            diff(&old, &new),
            vec![
                rename("a", "brocanth_tmp_f1"),
                rename("b", "brocanth_tmp_f2"),
                rename("brocanth_tmp_f1", "b"),
                rename("brocanth_tmp_f2", "a"),
            ]
        );
        let tables = diff(
            &Schema::new(vec![table("T1", "x", vec![]), table("T2", "y", vec![])]),
            &Schema::new(vec![table("T1", "y", vec![]), table("T2", "x", vec![])]),
        );
        assert_eq!(tables.len(), 4, "{tables:?}");
        for step in &tables[..2] {
            assert!(
                matches!(step, Change::RenameTable { to, .. } if to.starts_with("brocanth_tmp_")),
                "{tables:?}"
            );
        }
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
        assert!(matches!(&changes[0], Change::DropTable(t) if t.name == "post"));
        assert!(matches!(&changes[1], Change::CreateTable(t) if t.name == "user"));
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
