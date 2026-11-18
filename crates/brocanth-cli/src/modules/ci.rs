//! `ci`: continuous integration, and `ci-github`, its first provider.
//!
//! A parent with one child today, because the checks are the same wherever
//! they run and the file that runs them is not. A second provider is a
//! second child beside `ci-github`, writing its own file from the same
//! list of steps.
//!
//! The parent writes nothing on its own. It is the line in the chooser that
//! says "check every push"; the provider under it writes the file.

use brocanth_scaffold::{Category, Manifest, Module, PgPool, Project, Scaffold};

pub struct Ci;

impl Module for Ci {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "ci",
            "fmt, clippy and the tests on every push; pick the provider below",
        )
        .category(Category::External)
        .outside_runtime()
        .asked_in("operations", "Continuous integration")
    }

    fn contribute(&self, _project: &Project, _s: &mut Scaffold) {}
}

pub struct GitHub;

impl Module for GitHub {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "ci-github",
            ".github/workflows/ci.yml: fmt, clippy, tests, a Postgres service when there is a database, the image",
        )
        .category(Category::External)
        .adapts_to("static", "CI keeps the exported site as an artifact")
        .adapts_to("docker", "CI builds the image")
        .adapts_to(
            "rls",
            "CI runs `rls diff`, which fails when `schema()` has a change no migration holds",
        )
        // A queue or a cache in Postgres requires the pool, so the pool is
        // the one question: no key list to keep in step.
        .uses::<PgPool>("CI starts a database for the tests")
        .outside_runtime()
        .asked_in("operations", "GitHub Actions")
        .part_of("ci")
        .because("it is one provider of the checks `ci` stands for")
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file(".github/workflows/ci.yml", workflow(project));
    }
}

/// Whether the project's tests want a database: it has the shared pool,
/// which every module that stores rows in Postgres requires.
fn wants_postgres(project: &Project) -> bool {
    project.provider::<PgPool>().is_some()
}

fn workflow(project: &Project) -> String {
    let mut out = String::from(
        r#"# Every push and every pull request: formatting, lints, tests.
#
# The toolchain is the one `rust-toolchain.toml` pins. A dependency on a path
# outside this repository (`brocanth create --brocanth-path`) is not in the
# checkout, and the build fails; depend on git instead.
name: ci

on:
  push:
    branches: [main]
  pull_request:

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

# Read-only by default. A step that needs more asks for it in its own job.
permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always

jobs:
  check:
    runs-on: ubuntu-latest
"#,
    );
    if wants_postgres(project) {
        out.push_str(
            r#"    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_PASSWORD: postgres
        ports: ["5432:5432"]
        options: >-
          --health-cmd pg_isready --health-interval 5s --health-timeout 5s --health-retries 10
"#,
        );
        // `[db] url`, as the config loader reads it from the environment. A
        // test that loads the configuration the way the server does opens
        // this database.
        out.push_str(&format!(
            "    env:\n      \
             # `[db] url` for the config loader, so a test that loads the\n      \
             # configuration as the server does reaches this service.\n      \
             {}_DB__URL: postgres://postgres:postgres@localhost:5432/postgres\n",
            project.ident().to_uppercase()
        ));
    }
    out.push_str(
        r#"    steps:
      - uses: actions/checkout@v4
      # Reads rust-toolchain.toml, and caches the target directory.
      - uses: actions-rust-lang/setup-rust-toolchain@v1
        with:
          components: rustfmt, clippy
      - run: cargo fmt --all --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test
"#,
    );
    if project.has("rls") {
        // Bridge decision 12: a rule edit without its migration moves the
        // application's `WHERE` and leaves the database on the old rule.
        // `rls diff` reads no database.
        out.push_str(
            r#"      # Fails when `schema()` in src/plugins/rls/mod.rs has a change that
      # migrations/rls.json does not hold: run `rls make <name>` and commit
      # the migration with the snapshot.
      - run: cargo run -- rls diff
"#,
        );
    }
    if project.has("static") {
        // The site is the build output, so CI keeps it. A deploy step to
        // a host reads this artifact.
        out.push_str(
            r#"      - run: cargo run -- export
      - uses: actions/upload-artifact@v4
        with:
          name: site
          path: dist
"#,
        );
    }
    if project.has("docker") {
        out.push_str(
            r#"
  image:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: docker/setup-buildx-action@v3
      # Builds the image and pushes nothing. Add a registry login and
      # `push: true` when there is somewhere to push to.
      - uses: docker/build-push-action@v6
        with:
          context: .
          push: false
          cache-from: type=gha
          cache-to: type=gha,mode=max
"#,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(on: &[&'static str]) -> String {
        let known = crate::modules::keys();
        let bindings = crate::resolve::bindings(on);
        workflow(&Project::new("my-blog", "My Blog", on, &known).with_bindings(&bindings))
    }

    #[test]
    fn the_workflow_token_is_read_only() {
        assert!(written(&[]).contains("permissions:\n  contents: read\n"));
    }

    #[test]
    fn a_project_with_rls_checks_its_snapshot() {
        let out = written(&["db", "commands", "rls"]);
        assert!(out.contains("- run: cargo run -- rls diff\n"), "{out}");
        assert!(!written(&["db", "commands"]).contains("rls diff"));
    }

    #[test]
    fn the_database_url_is_the_variable_the_config_loader_reads() {
        let out = written(&["db"]);
        assert!(out.contains("MY_BLOG_DB__URL: postgres://"), "{out}");
        assert!(!out.contains("BROCANTH_TEST_DATABASE_URL"), "{out}");
        assert!(!written(&[]).contains("postgres"));
    }
}
