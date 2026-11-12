//! The `rls` module, written into a project by the CLI's generator. Bless
//! with `BROCANTH_BLESS=1`; build and test each project too with
//! `BROCANTH_BUILD=1`.

use brocanth_rls::scaffold::Rls;
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&Rls])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&Rls])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// SQL migrations: `rls make` writes a `.sql` file that `migrate up`
/// applies, and `rls check` reads `[db]` from the config file.
#[test]
fn a_db_project_writes_sql_migrations() {
    shape("rls", &["config", "db", "commands", "rls"]);
}

/// With `model`, the file is a SeaORM migration; without `config`, the URL
/// comes from the environment.
#[test]
fn a_model_project_writes_seaorm_migrations() {
    shape("rls-model", &["db", "model", "commands", "rls"]);
}

/// Without `commands` there is nowhere to run `rls make`.
#[test]
fn rls_without_commands_is_refused_with_the_reason() {
    let e =
        brocanth_cli::testing::generate("my-blog", &["db", "rls"], &[&Rls]).expect_err("refused");
    assert!(e.contains("rls make"), "{e}");
}
