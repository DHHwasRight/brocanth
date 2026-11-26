//! `ci`: continuous integration, and `ci-github`, its first provider.
//!
//! A parent with one child today, because the checks are the same wherever
//! they run and the file that runs them is not. A second provider is a
//! second child beside `ci-github`, writing its own file from the same
//! list of steps.
//!
//! The parent writes nothing on its own. It is the line in the chooser that
//! says "check every push"; the provider under it writes the file.
//!
//! `ci-security` is a second child: the dependency checks, `cargo audit`
//! and `cargo deny`, in a workflow of their own.

use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Manifest, Module, PgPool, Project, Scaffold, Slot,
};

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

/// The dependency checks: `cargo audit` and `cargo deny`, in
/// `.github/workflows/security.yml`, with the policy in `deny.toml`.
///
/// A workflow of its own and not a job in `ci.yml`, for one reason: it
/// also runs each day. A new advisory arrives without a push, and a check
/// that runs only on a push finds it at the next change, which may be weeks
/// later.
pub struct Security;

impl Module for Security {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "ci-security",
            ".github/workflows/security.yml: cargo audit and cargo deny on each push and each day; deny.toml",
        )
        .category(Category::External)
        .needs(
            "ci-github",
            "the checks are a GitHub Actions workflow, and without the provider nothing runs them",
        )
        .contributes::<AgentsSection>(
            Contribution::new(
                "ci-security",
                "how a CI step installs a tool, and what to do with an advisory",
            ),
            true,
        )
        .outside_runtime()
        .asked_in(
            "operations",
            "Check dependencies for advisories, bans and sources",
        )
        // Off with its parent: a new advisory fails the daily run, and a
        // team chooses when it wants that mail.
        .part_of_but_off("ci")
        .because("it is one more set of checks that `ci` runs")
        .absent(
            "ci-security",
            "no dependency audit; a crate with a published advisory ships unnoticed",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        s.file(".github/workflows/security.yml", SECURITY_WORKFLOW);
        s.file("deny.toml", DENY_TOML);
        s.at(Slot::AgentsSections).block(SECURITY_AGENTS);
    }
}

/// `.github/workflows/security.yml`.
///
/// Two tools, two ways to install, and both pinned:
///
/// - `cargo-audit` publishes no checksum with its binaries, so it is built
///   with `cargo install --locked` at a fixed version. Cargo checks each
///   crate it downloads against the crates.io index checksum, and
///   `--locked` builds the lock file the release was published with.
/// - `cargo-deny` publishes a `.sha256` file beside each archive. The step
///   downloads the archive to a file, checks it with `sha256sum -c`, and
///   only then unpacks it. Nothing is piped into `tar` or a shell.
const SECURITY_WORKFLOW: &str = r#"# The dependency checks: RustSec advisories (`cargo audit`), and the bans and
# sources in deny.toml (`cargo deny`). On each push to main, on each pull
# request, and each day, because a new advisory arrives without a push.
name: security

on:
  push:
    branches: [main]
  pull_request:
  schedule:
    - cron: "17 6 * * *"
  workflow_dispatch:

concurrency:
  group: security-${{ github.ref }}
  cancel-in-progress: true

permissions:
  contents: read

env:
  CARGO_TERM_COLOR: always

jobs:
  audit:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions-rust-lang/setup-rust-toolchain@v1
      # Built from crates.io at a fixed version. Cargo checks each crate it
      # downloads against the index checksum, and `--locked` uses the lock
      # file the release was published with. To bump: change the version.
      - run: cargo install cargo-audit --version 0.23.0 --locked
      # Fails on an advisory against a crate in Cargo.lock. To accept one,
      # add its id to `ignore` in deny.toml with the reason, and pass
      # `--ignore <id>` here.
      - run: cargo audit

  deny:
    runs-on: ubuntu-latest
    env:
      # Bump both together. The hash is the one in the release's
      # `.sha256` file:
      # https://github.com/EmbarkStudios/cargo-deny/releases
      CARGO_DENY_VERSION: "0.20.2"
      CARGO_DENY_SHA256: "9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f"
    steps:
      - uses: actions/checkout@v4
      - uses: actions-rust-lang/setup-rust-toolchain@v1
      - name: Install cargo-deny, checked against its SHA-256
        run: |
          set -euo pipefail
          name="cargo-deny-${CARGO_DENY_VERSION}-x86_64-unknown-linux-musl"
          archive="$RUNNER_TEMP/$name.tar.gz"
          # To a file, checked, then unpacked: never piped into `tar`.
          curl --fail --silent --show-error --location --output "$archive" \
            "https://github.com/EmbarkStudios/cargo-deny/releases/download/${CARGO_DENY_VERSION}/$name.tar.gz"
          echo "${CARGO_DENY_SHA256}  $archive" | sha256sum -c -
          tar -xzf "$archive" -C "$RUNNER_TEMP"
          mkdir -p "$RUNNER_TEMP/bin"
          install -m 0755 "$RUNNER_TEMP/$name/cargo-deny" "$RUNNER_TEMP/bin/cargo-deny"
          echo "$RUNNER_TEMP/bin" >> "$GITHUB_PATH"
      # Licenses are not checked: see `[licenses]` in deny.toml.
      - run: cargo deny check advisories bans sources
"#;

/// `deny.toml`: the policy `cargo deny` reads.
const DENY_TOML: &str = r#"# The policy `cargo deny check` reads. CI runs it in
# .github/workflows/security.yml. The reference:
# https://embarkstudios.github.io/cargo-deny/checks/cfg.html

[graph]
# Every feature on, so a crate that only one feature pulls in is checked
# too.
all-features = true

[advisories]
# A yanked crate in Cargo.lock fails the check: its author withdrew it.
yanked = "deny"
# An unmaintained crate fails the check when this project depends on it
# directly. One deeper in the tree is its dependency's to replace, and
# `cargo audit` still lists it as a warning.
unmaintained = "workspace"
# An advisory you accept, with the reason and when to look again:
# { id = "RUSTSEC-0000-0000", reason = "not reachable: we never call X" }
ignore = []

[bans]
# Two versions of one crate build twice and link twice. A warning, because
# it is often a dependency's choice and not yours.
multiple-versions = "warn"
# `*` as a version takes whatever is published next. A warning, not an
# error: brocanth's crates are git dependencies with no version, and
# `cargo deny` reads a missing version as `*`. The commit in Cargo.lock
# pins them.
wildcards = "warn"

[sources]
# A crate comes from crates.io or from one of the git hosts below. Any other
# registry or repository fails the check.
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
# brocanth and its template engines are git dependencies.
allow-org = { github = ["DHHwasRight"] }

[licenses]
# Not run in CI yet: brocanth's crates are MIT, but this project and the
# `zod` crate declare no `license` in Cargo.toml, and `cargo deny` fails a
# crate with none. When every crate declares one, add `licenses` to the
# `cargo deny check` step.
allow = [
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "CDLA-Permissive-2.0",
    "ISC",
    "MIT",
    "Unicode-3.0",
    "Zlib",
]
confidence-threshold = 0.9
"#;

/// What the generated `AGENTS.md` says about the dependency checks.
const SECURITY_AGENTS: &str = "\n\
    ## Dependency checks\n\
    \n\
    `.github/workflows/security.yml` runs `cargo audit` and `cargo deny` on\n\
    each push and each day. `deny.toml` is the policy.\n\
    \n\
    - A CI step that downloads a tool pins its version and its SHA-256,\n\
    \x20 downloads to a file, and checks it with `sha256sum -c` before it\n\
    \x20 unpacks or runs it. Take the hash from the release, not from your\n\
    \x20 own download. Never pipe a download into `sh`, `bash` or `tar`.\n\
    \x20 `cargo install --locked --version X` and an action pinned by commit\n\
    \x20 SHA need no extra check.\n\
    - An advisory fails the check. Update the crate (`cargo update -p\n\
    \x20 <crate>`). Accept one only in `ignore` in `deny.toml`, with the\n\
    \x20 reason, and pass the same id to `cargo audit --ignore`.\n\
    - A new git dependency outside the allowed hosts fails `sources`. Add\n\
    \x20 its host to `[sources]` only after you read who publishes it.\n";

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
      # the migration with the snapshot. Before the first `rls make` there
      # is no snapshot, and the step says so instead of failing.
      - name: rls diff
        run: |
          if [ -f migrations/rls.json ]; then
            cargo run -- rls diff
          else
            echo "!! SKIPPED rls diff: no migrations/rls.json yet. help: run \`brocanth rls make init\` and commit the migration"
          fi
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
    fn every_download_in_the_security_workflow_is_pinned_and_checked() {
        let out = SECURITY_WORKFLOW;
        // Each `curl` writes a file, which `sha256sum -c` checks before
        // `tar` reads it. A pipe would skip the check.
        assert_eq!(out.matches("curl ").count(), 1, "{out}");
        for piped in out.split('|').skip(1) {
            let tool = piped.split_whitespace().next().unwrap_or_default();
            assert!(!["sh", "bash", "tar", "python"].contains(&tool), "{out}");
        }
        let curl = out.find("curl ").expect("a download");
        let check = out.find("sha256sum -c").expect("a hash check");
        let unpack = out.find("tar -xzf").expect("an unpack");
        assert!(curl < check && check < unpack, "{out}");
        // The version and its hash sit side by side, and the hash is whole.
        let hash = out
            .lines()
            .find_map(|l| l.trim().strip_prefix("CARGO_DENY_SHA256: "))
            .expect("a pinned hash")
            .trim_matches('"');
        assert_eq!(hash.len(), 64);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(out.contains("CARGO_DENY_VERSION: \"0.20.2\""));
        // A tool built from crates.io is pinned and locked.
        assert!(out.contains("cargo install cargo-audit --version 0.23.0 --locked"));
    }

    #[test]
    fn the_security_checks_run_each_day_and_read_the_policy() {
        assert!(SECURITY_WORKFLOW.contains("schedule:\n    - cron: "));
        assert!(SECURITY_WORKFLOW.contains("permissions:\n  contents: read\n"));
        assert!(DENY_TOML.contains("unknown-git = \"deny\""));
    }

    #[test]
    fn the_workflow_token_is_read_only() {
        assert!(written(&[]).contains("permissions:\n  contents: read\n"));
    }

    #[test]
    fn a_project_with_rls_checks_its_snapshot() {
        let out = written(&["db", "commands", "rls"]);
        assert!(out.contains("            cargo run -- rls diff\n"), "{out}");
        assert!(out.contains("if [ -f migrations/rls.json ]"), "{out}");
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
