//! The snapshot: what the models looked like when the last migration was
//! written. Checked in at `migrations/schema.json`, keyed by id.

use serde::{Deserialize, Serialize};

/// The snapshot format this crate reads and writes.
pub const FORMAT: u32 = 1;

/// Every table, sorted by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    /// [`FORMAT`].
    pub format: u32,
    /// The tables, sorted by name so the file diffs well.
    pub tables: Vec<Table>,
}

impl Default for Schema {
    fn default() -> Self {
        Self {
            format: FORMAT,
            tables: Vec::new(),
        }
    }
}

/// One model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Table {
    /// The id from `#[brocanth(id = "..")]` on the struct.
    pub id: String,
    /// The table name.
    pub name: String,
    /// The columns, in declaration order.
    pub fields: Vec<Field>,
}

/// One column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    /// The id from `#[brocanth(id = "..")]` on the field.
    pub id: String,
    /// The column name.
    pub name: String,
    /// The `ColumnDef` method for the type: `string`, `integer`, `text`, ..
    pub kind: String,
    /// `Option<T>`, or `#[sea_orm(nullable)]`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub nullable: bool,
    /// `#[sea_orm(primary_key)]`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub primary_key: bool,
    /// An integer primary key, unless `auto_increment = false`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_increment: bool,
    /// `#[sea_orm(unique)]`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unique: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Field {
    /// `true` when the column definition is the same, whatever the name.
    pub fn same_definition(&self, other: &Field) -> bool {
        self.kind == other.kind
            && self.nullable == other.nullable
            && self.primary_key == other.primary_key
            && self.auto_increment == other.auto_increment
            && self.unique == other.unique
    }
}

impl Schema {
    /// A schema from tables in any order.
    pub fn new(mut tables: Vec<Table>) -> Self {
        tables.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            format: FORMAT,
            tables,
        }
    }

    /// Read a snapshot.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let schema: Schema = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if schema.format != FORMAT {
            return Err(format!(
                "the snapshot has format {}; this tool reads format {FORMAT}",
                schema.format
            ));
        }
        Ok(schema)
    }

    /// Write a snapshot, with a final newline.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("a schema serialises");
        text.push('\n');
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_reads_back_what_it_wrote() {
        let schema = Schema::new(vec![Table {
            id: "01JBQ7R8V4K2M9X3T6N5P0WZ1A".into(),
            name: "post".into(),
            fields: vec![Field {
                id: "01JBQ7R8V4K2M9X3T6N5P0WZ1B".into(),
                name: "id".into(),
                kind: "integer".into(),
                nullable: false,
                primary_key: true,
                auto_increment: true,
                unique: false,
            }],
        }]);
        let text = schema.to_json();
        assert!(
            !text.contains("nullable"),
            "false flags are left out:\n{text}"
        );
        assert_eq!(Schema::from_json(&text).unwrap(), schema);
    }

    #[test]
    fn an_unknown_format_is_refused() {
        let err = Schema::from_json(r#"{"format": 9, "tables": []}"#).unwrap_err();
        assert!(err.contains("format 9"), "{err}");
    }
}
