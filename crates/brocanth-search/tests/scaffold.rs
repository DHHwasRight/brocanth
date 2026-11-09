//! The `search` module, written into a project by the CLI's generator,
//! checked with rustfmt and compared with a golden file. Bless with
//! `BROCANTH_BLESS=1`; build each project too with `BROCANTH_BUILD=1`.

use brocanth_scaffold::testing;
use brocanth_search::scaffold::{Search, MIGRATION_SQL};

fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&Search])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&Search])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// With rsx and a SQL migration: the page, its template and the table.
/// `policy` and `filters` too, so one build covers the three list modules.
#[test]
fn an_rsx_project_gets_a_search_page_and_a_sql_migration() {
    shape(
        "search-rsx",
        &["rsx", "db", "policy", "pagination", "filters", "search"],
    );
}

/// Without rsx, and with `model`: the page writes HTML, and the migration is
/// a SeaORM file.
#[test]
fn a_model_project_gets_a_seaorm_migration() {
    shape("search-model", &["db", "model", "pagination", "search"]);
}

/// The migration the module writes is the one the crate declares, so the
/// generated column and the query agree on the language and the weights.
#[test]
fn the_written_migration_is_the_crate_s() {
    assert_eq!(
        MIGRATION_SQL,
        brocanth_search::documents::migration().file(),
        "help: copy `documents::migration().file()` into MIGRATION_SQL"
    );
}

/// Without `db` the module is refused, with the reason.
#[test]
fn search_without_a_database_is_refused() {
    let wrong = brocanth_cli::testing::generate("my-blog", &["pagination", "search"], &[&Search])
        .expect_err("search needs db");
    assert!(wrong.contains("db"), "{wrong}");
}
