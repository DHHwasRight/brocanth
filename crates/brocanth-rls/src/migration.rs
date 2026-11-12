//! The migration file, and `rls make`: snapshot, diff, file.
//!
//! The file is a `brocanth-db` SQL migration
//! (`migrations/<YYYYMMDD_HHMMSS>_<name>.sql`), or, in a project with
//! `model`, a SeaORM migration (`migrations/m<YYYYMMDD_HHMMSS>_<name>.rs`)
//! that runs the same SQL. `migrations/rls.json` records the schema the
//! file leads to.

use std::path::{Path, PathBuf};

use crate::diff::{diff, down, Plan};
use crate::error::RlsError;
use crate::schema::Schema;
use crate::snapshot::Snapshot;

/// The snapshot file, in the migrations directory.
pub const SNAPSHOT: &str = "rls.json";

/// Which migration path the project has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `brocanth-db`'s SQL files, for a project with `db` and no `model`.
    Sql,
    /// A SeaORM migration file, for a project with `model`.
    SeaOrm,
}

/// What [`make`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    /// The migration file.
    pub path: PathBuf,
    /// What it does, one line per step.
    pub steps: Vec<String>,
}

const HEADER: &str = "-- Written by `rls make`. Read it before you commit it.\n\
     -- migrations/rls.json records the schema; this file is the step to it.\n";

/// The SQL migration: the up plan, a `-- down` line, the down plan.
///
/// # Errors
/// [`RlsError::Definition`] when a statement holds a line `-- down`, which
/// would split the file in the wrong place.
pub fn sql_file(up: &Plan, down: &Plan) -> Result<String, RlsError> {
    let (up, down) = (up.sql(), down.sql());
    for line in up.lines().chain(down.lines()) {
        if line
            .trim()
            .eq_ignore_ascii_case(brocanth_db::migrate::DOWN_MARKER)
        {
            return Err(RlsError::definition(
                "a statement holds the line `-- down`, which splits a migration file",
                "remove the line break and the `-- down` text from the value in the expression",
            ));
        }
    }
    Ok(format!("{HEADER}\n{up}\n-- down\n{down}"))
}

/// The SeaORM migration: the same statements, run with
/// `execute_unprepared`. `name` is the file stem, `m<UTC>_<name>`.
pub fn seaorm_file(name: &str, up: &Plan, down: &Plan) -> String {
    let (up, down) = (up.sql(), down.sql());
    let mut hashes = String::from("#");
    while up.contains(&format!("\"{hashes}")) || down.contains(&format!("\"{hashes}")) {
        hashes.push('#');
    }
    format!(
        "// Written by `rls make`. Read it before you commit it.\n\
         // migrations/rls.json records the schema; this file is the step to it.\n\
         \n\
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
         impl MigrationTrait for Migration {{\n\
         \x20   /// One transaction, so no request sees a table between a\n\
         \x20   /// dropped policy and its new one.\n\
         \x20   fn use_transaction(&self) -> Option<bool> {{\n\
         \x20       Some(true)\n\
         \x20   }}\n\
         \n\
         \x20   async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {{\n\
         \x20       manager.get_connection().execute_unprepared(UP).await?;\n\
         \x20       Ok(())\n\
         \x20   }}\n\
         \n\
         \x20   async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {{\n\
         \x20       manager.get_connection().execute_unprepared(DOWN).await?;\n\
         \x20       Ok(())\n\
         \x20   }}\n\
         }}\n\
         \n\
         const UP: &str = r{hashes}\"\n{up}\"{hashes};\n\
         \n\
         const DOWN: &str = r{hashes}\"\n{down}\"{hashes};\n"
    )
}

/// Reads `dir/rls.json`, or an empty snapshot when there is none.
///
/// # Errors
/// [`RlsError::Io`] or [`RlsError::Snapshot`].
pub fn read_snapshot(dir: &Path) -> Result<Snapshot, RlsError> {
    let path = dir.join(SNAPSHOT);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            Snapshot::from_json(&text).map_err(|source| RlsError::Snapshot { path, source })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Snapshot::default()),
        Err(source) => Err(RlsError::Io {
            path,
            source,
            help: "check that the migrations directory is readable",
        }),
    }
}

/// `rls make <name>`: compares `schema` with `dir/rls.json`, writes the
/// migration and the new snapshot. `None` when nothing changed: no file is
/// written.
///
/// # Errors
/// What [`Schema::snapshot`] refuses; a name that is not snake case; a file
/// that cannot be read or written.
pub fn make(
    schema: &Schema,
    dir: &Path,
    name: &str,
    format: Format,
) -> Result<Option<Made>, RlsError> {
    let old = read_snapshot(dir)?;
    let new = schema.snapshot()?;
    let up = diff(&old, &new)?;
    if up.is_empty() {
        return Ok(None);
    }
    let down = down(&new, &old)?;
    let sql = sql_file(&up, &down)?;
    // `new_file` checks the name and stamps it, as `migrate new` does.
    let path = brocanth_db::migrate::new_file(dir, name)?;
    let path = match format {
        Format::Sql => {
            write(&path, &sql)?;
            path
        }
        Format::SeaOrm => {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(name)
                .to_string();
            let _ = std::fs::remove_file(&path);
            let rs = dir.join(format!("m{stem}.rs"));
            write(&rs, &seaorm_file(&format!("m{stem}"), &up, &down))?;
            rs
        }
    };
    write(&dir.join(SNAPSHOT), &new.to_json())?;
    Ok(Some(Made {
        path,
        steps: up.describe(),
    }))
}

fn write(path: &Path, text: &str) -> Result<(), RlsError> {
    std::fs::write(path, text).map_err(|source| RlsError::Io {
        path: path.to_path_buf(),
        source,
        help: "check that the migrations directory exists and is writable",
    })
}
