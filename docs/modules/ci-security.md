# `ci-security`

.github/workflows/security.yml: cargo audit and cargo deny on each push and each day; deny.toml.

The wizard calls it: *Check dependencies for advisories, bans and sources*.

| | |
|---|---|
| Categories | **External** |
| Wizard question | Operations |
| Default | off |
| Presets | none |
| Part of | [`ci`](ci.md), off until you tick it |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | none |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/ci_security.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`ci-github`](ci-github.md): the checks are a GitHub Actions workflow, and without the provider nothing runs them |
| After | [`ci-github`](ci-github.md) |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | `AgentsSection` `ci-security`: how a CI step installs a tool, and what to do with an advisory |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `.github/workflows/security.yml`: two jobs, on each push to `main`, on
  each pull request, each day at 06:17 UTC, and on demand.
  - `audit`: `cargo audit` against the RustSec database. It fails on an
    advisory against a crate in `Cargo.lock`.
  - `deny`: `cargo deny check advisories bans sources`, with the policy
    in `deny.toml`.
- `deny.toml`: yanked crates fail, a `*` version fails, two versions of
  one crate warn, and a crate must come from crates.io or a GitHub
  repository of `DHHwasRight` (brocanth, `rsx`, `jt`).

Slots:

- AGENTS.md: `## Dependency checks`

**Each day, not only on a push.** A new advisory arrives without a change
to your code. A check that runs only on a push finds it at the next push,
which can be weeks later. The scheduled run fails and GitHub mails the
repository's owners.

**Every tool is pinned, and nothing is piped into a shell.** A CI step
that installs a tool is code that runs with the repository's token.

- `cargo-audit` publishes no checksum beside its binaries. The job builds
  it with `cargo install cargo-audit --version 0.23.0 --locked`. Cargo
  checks every crate it downloads against the crates.io index checksum,
  and `--locked` builds the lock file of the release.
- `cargo-deny` publishes a `.sha256` file beside each archive. The job
  pins the version and the hash in two variables, side by side, downloads
  the archive to a file, checks it with `sha256sum -c`, and unpacks it
  only after the check passes. A wrong hash stops the job before `tar`
  runs. To bump: change both variables, and take the hash from the
  release's `.sha256` file, not from your own download.

**Licenses are not checked yet.** brocanth's crates are MIT, but the
generated project declares no `license` in `Cargo.toml`, and neither does
the `zod` crate it builds on. `cargo deny` fails a crate with none. `deny.toml` has a `[licenses]` allow-list ready. Add
`licenses` to the `cargo deny check` step when every crate declares one.

**Why two tools.** `cargo audit` is the RustSec reference and reads
`Cargo.lock` alone. `cargo deny` adds what `cargo audit` does not check:
where a crate comes from, wildcard versions and duplicates. Both read the
same advisory database, so an advisory you accept goes in `ignore` in
`deny.toml` and in `cargo audit --ignore`.

## Settings

None. The policy is `deny.toml`, and the schedule is in the workflow.

## Read more

- [`ci-github`](ci-github.md), which this module needs
- Source: [`crates/brocanth-cli/src/modules/ci.rs`](../../crates/brocanth-cli/src/modules/ci.rs)
