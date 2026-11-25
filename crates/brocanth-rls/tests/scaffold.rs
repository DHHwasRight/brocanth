//! The `rls` module, written into a project by the CLI's generator. Bless
//! with `BROCANTH_BLESS=1`; build and test each project too with
//! `BROCANTH_BUILD=1`.

use brocanth_rls::scaffold::Rls;
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) -> String {
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
    manifest(&files)
}

fn manifest(files: &[(String, String)]) -> String {
    files
        .iter()
        .find(|(path, _)| path == "Cargo.toml")
        .map(|(_, text)| text.clone())
        .expect("a Cargo.toml")
}

/// The lines of one table of `manifest`, `[dependencies]` for example.
fn table<'a>(manifest: &'a str, name: &str) -> Vec<&'a str> {
    manifest
        .lines()
        .skip_while(|line| *line != name)
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .collect()
}

/// SQL migrations: `rls make` writes a `.sql` file that `migrate up`
/// applies, and `rls check` reads `[db]` from the config file.
#[test]
fn a_db_project_writes_sql_migrations() {
    let manifest = shape("rls", &["config", "db", "commands", "rls"]);
    // Without `policy` there is no agreement test, so no dev line.
    assert!(!manifest.contains("[dev-dependencies]"), "{manifest}");
}

/// With `model`, the file is a SeaORM migration; without `config`, the URL
/// comes from the environment.
#[test]
fn a_model_project_writes_seaorm_migrations() {
    shape("rls-model", &["db", "model", "commands", "rls"]);
}

/// With `policy`, the `RowSecurity` plugin puts the request's identity on
/// the context; with `admin-model` too, the admin runs as `staff`; with
/// `resource`, AGENTS.md says how a resource takes the identity.
#[test]
fn a_policy_project_builds_the_request_identity() {
    let manifest = shape(
        "rls-policy",
        &[
            "config",
            "db",
            "model",
            "commands",
            "policy",
            "resource",
            "admin",
            "admin-model",
            "rls",
        ],
    );
    // The agreement test needs `agreement`, and only `cargo test` compiles
    // it: the feature is on the dev line, and the binary's line has none.
    let rls = |name: &str| {
        table(&manifest, name)
            .into_iter()
            .find(|line| line.starts_with("brocanth-rls = "))
            .unwrap_or_else(|| panic!("no brocanth-rls in {name}:\n{manifest}"))
            .to_string()
    };
    assert!(!rls("[dependencies]").contains("agreement"), "{manifest}");
    assert!(
        rls("[dev-dependencies]").contains("\"agreement\""),
        "{manifest}"
    );
}

/// Without `commands` there is nowhere to run `rls make`.
#[test]
fn rls_without_commands_is_refused_with_the_reason() {
    let e =
        brocanth_cli::testing::generate("my-blog", &["db", "rls"], &[&Rls]).expect_err("refused");
    assert!(e.contains("rls make"), "{e}");
}
