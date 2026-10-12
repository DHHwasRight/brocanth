//! The adapter: what a SeaORM schema becomes before the admin sees it.

use brocanth_admin::adapter::{column_from_schema, entry_from_schema, SchemaColumn};
use brocanth_admin::{ColumnKind, Widget};

fn column<'a>(name: &'a str, type_name: &'a str) -> SchemaColumn<'a> {
    SchemaColumn {
        name,
        type_name,
        nullable: false,
        unique: false,
        primary_key: false,
    }
}

#[test]
fn a_schema_column_keeps_what_the_schema_said() {
    let described = column_from_schema(&SchemaColumn {
        nullable: true,
        unique: true,
        ..column("email", "string")
    });

    assert_eq!(described.kind, ColumnKind::Text);
    assert!(described.nullable);
    assert!(described.unique);
    assert!(!described.primary_key);
    // Neither relations nor enum variants nor a length are in SeaORM's column
    // schema, so they arrive empty and are declared in the configuration.
    assert!(described.relation.is_none());
    assert!(described.variants.is_empty());
    assert_eq!(described.max_length, None);
}

#[test]
fn a_single_integer_key_is_taken_to_be_generated() {
    let entry = entry_from_schema(
        "blog::models::post",
        "post",
        &[
            SchemaColumn {
                primary_key: true,
                ..column("id", "integer")
            },
            column("title", "string"),
        ],
    );

    assert_eq!(entry.key(), "post");
    let id = &entry.columns[0];
    assert!(id.primary_key);
    assert!(id.auto_generated);
    assert_eq!(
        brocanth_admin::widget_for(id),
        Widget::Hidden,
        "a generated key is never asked for"
    );
}

#[test]
fn a_composite_key_is_asked_for() {
    let entry = entry_from_schema(
        "blog::models::tagging",
        "tagging",
        &[
            SchemaColumn {
                primary_key: true,
                ..column("post_id", "integer")
            },
            SchemaColumn {
                primary_key: true,
                ..column("tag_id", "integer")
            },
        ],
    );

    for column in &entry.columns {
        assert!(column.primary_key);
        assert!(!column.auto_generated, "nothing generates a composite key");
        assert_eq!(brocanth_admin::widget_for(column), Widget::Number);
    }
}

#[test]
fn a_text_key_is_asked_for_too() {
    let entry = entry_from_schema(
        "blog::models::setting",
        "setting",
        &[SchemaColumn {
            primary_key: true,
            ..column("slug", "string")
        }],
    );

    assert!(!entry.columns[0].auto_generated);
    assert_eq!(brocanth_admin::widget_for(&entry.columns[0]), Widget::Text);
}

#[test]
fn the_entry_key_falls_back_to_the_table_without_a_module_path() {
    let entry = entry_from_schema("", "post", &[]);
    assert_eq!(entry.key(), "post");
}
