//! Each module's manifest names exactly the tables its crate creates.
//!
//! `Manifest::tables` feeds `brocanth remove`: it prints the `DROP TABLE`
//! statement for a leaving module's tables. The list is written by hand
//! beside the crate's `CREATE TABLE` statements, so a table added to the SQL
//! and forgotten in the manifest outlives its module, and a name in the
//! manifest that the SQL does not create drops nothing.
//!
//! One check for every module, so a new module cannot be left out:
//!
//! * Each module in the CLI's list that declares a table must have a row in
//!   [`SOURCES`].
//! * Each row names a module the CLI has.
//! * For each row, the `CREATE [UNLOGGED] TABLE [IF NOT EXISTS] <name>`
//!   statements in the row's source files, minus the shared `queue_*`
//!   tables, name exactly the tables the manifest declares.
//!
//! The source files are the files that hold the SQL the crate executes:
//! the string constants, not a copy. A comment line is skipped, so a
//! statement quoted in a doc comment does not count.

use std::collections::BTreeSet;
use std::path::Path;

/// One module that creates tables: the module key and the files, relative
/// to `crates/`, that hold the statements it runs.
struct Row {
    key: &'static str,
    files: &'static [&'static str],
}

/// Every module of the CLI that creates a table of its own.
const SOURCES: &[Row] = &[
    Row {
        key: "analytics",
        files: &["brocanth-analytics/src/postgres.rs"],
    },
    Row {
        key: "audit",
        files: &["brocanth-audit/src/postgres.rs"],
    },
    Row {
        key: "auth",
        files: &["brocanth-auth/src/postgres/sql.rs"],
    },
    Row {
        key: "cache",
        files: &["brocanth-cache/src/postgres/sql.rs"],
    },
    Row {
        key: "flags",
        files: &["brocanth-flags/src/postgres.rs"],
    },
    Row {
        key: "notifications",
        files: &["brocanth-notifications/src/store.rs"],
    },
    // The queue's own tables are `queue_*`, shared with every queue user.
    Row {
        key: "jobs",
        files: &["brocanth-jobs/src/schedule.rs"],
    },
    // `PgPending`, which the mail plugin uses when the project has a pool.
    Row {
        key: "mail",
        files: &["brocanth-mail/src/pending.rs"],
    },
    Row {
        key: "oauth-client",
        files: &["brocanth-oauth-client/src/postgres.rs"],
    },
    // The migration the scaffold writes is `documents::migration()`.
    Row {
        key: "search",
        files: &["brocanth-search/src/documents.rs"],
    },
    Row {
        key: "storage",
        files: &["brocanth-storage/src/postgres/sql.rs"],
    },
    Row {
        key: "webhooks-in",
        files: &["brocanth-webhooks/src/postgres.rs"],
    },
    Row {
        key: "webhooks-out",
        files: &["brocanth-webhooks/src/outgoing.rs"],
    },
];

/// The table names after `CREATE [UNLOGGED] TABLE [IF NOT EXISTS]` in
/// `source`, without quotes. Comment lines are skipped.
fn created(source: &str) -> BTreeSet<String> {
    let code: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let words: Vec<&str> = code.split_whitespace().collect();
    let is = |j: usize, word: &str| words.get(j).is_some_and(|w| w.eq_ignore_ascii_case(word));
    let mut names = BTreeSet::new();
    let mut i = 0;
    while i < words.len() {
        // `"CREATE` or `r#"CREATE` at the start of a string constant.
        let first = words[i].trim_start_matches(['"', '\'', '#', 'r']);
        let mut j = i + 1;
        if is(j, "UNLOGGED") {
            j += 1;
        }
        if !first.eq_ignore_ascii_case("CREATE") || !is(j, "TABLE") {
            i += 1;
            continue;
        }
        j += 1;
        if is(j, "IF") && is(j + 1, "NOT") && is(j + 2, "EXISTS") {
            j += 3;
        }
        if let Some(raw) = words.get(j) {
            let name: String = raw
                .chars()
                .filter(|c| !matches!(c, '"' | '\\'))
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                names.insert(name);
            }
        }
        i = j;
    }
    names
}

#[test]
fn created_reads_each_form_of_the_statement() {
    let source = "// CREATE TABLE in_a_comment (\n\
                  const A: &str = \"CREATE TABLE IF NOT EXISTS a (\n\
                  CREATE UNLOGGED TABLE IF NOT EXISTS b (\n\
                  CREATE TABLE \\\"c\\\" (\n\
                  create table d(id int)\";";
    let names: Vec<String> = created(source).into_iter().collect();
    assert_eq!(names, ["a", "b", "c", "d"]);
}

#[test]
fn every_module_that_declares_a_table_has_a_row() {
    let missing: Vec<&str> = brocanth_cli::testing::modules()
        .iter()
        .map(|m| m.manifest())
        .filter(|m| !m.tables.is_empty())
        .map(|m| m.key)
        .filter(|key| !SOURCES.iter().any(|row| row.key == *key))
        .collect();
    assert!(
        missing.is_empty(),
        "these modules declare tables and have no row in SOURCES: {missing:?}\n\
         help: add a row in crates/brocanth-cli/tests/manifest_tables.rs that names the file with the module's CREATE TABLE statements"
    );
}

#[test]
fn every_row_names_a_module_the_cli_has() {
    let keys: Vec<&str> = brocanth_cli::testing::modules()
        .iter()
        .map(|m| m.manifest().key)
        .collect();
    for row in SOURCES {
        assert!(
            keys.contains(&row.key),
            "SOURCES has `{}`, which no module has\n\
             help: correct the key, or remove the row with the module",
            row.key
        );
    }
}

#[test]
fn every_manifest_declares_exactly_the_tables_its_crate_creates() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut drift = Vec::new();
    for row in SOURCES {
        let module = brocanth_cli::testing::modules()
            .iter()
            .find(|m| m.manifest().key == row.key)
            .expect("every_row_names_a_module_the_cli_has");
        let declared: BTreeSet<String> = module
            .manifest()
            .tables
            .iter()
            .map(|t| t.to_string())
            .collect();
        let mut sql = BTreeSet::new();
        for file in row.files {
            let path = crates.join(file);
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            sql.extend(created(&source));
        }
        sql.retain(|t| !t.starts_with("queue_"));
        assert!(
            !sql.is_empty(),
            "`{}`: no CREATE TABLE statement in {:?}\n\
             help: point the row in crates/brocanth-cli/tests/manifest_tables.rs at the file that holds the SQL",
            row.key,
            row.files
        );
        let unlisted: Vec<&String> = sql.difference(&declared).collect();
        let uncreated: Vec<&String> = declared.difference(&sql).collect();
        if !unlisted.is_empty() || !uncreated.is_empty() {
            drift.push(format!(
                "`{}` ({:?}): created and not declared {unlisted:?}, declared and not created {uncreated:?}",
                row.key, row.files
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "a manifest's tables do not match its crate's CREATE TABLE statements:\n{}\n\
         help: make each module's `Manifest::tables` list exactly the tables its files create, minus the shared queue_* tables",
        drift.join("\n")
    );
}
