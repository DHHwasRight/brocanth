# `ci-security`

.github/workflows/security.yml: cargo audit and cargo deny on each push and each day; deny.toml.

The wizard calls it: *Check dependencies for advisories, bans, licenses and sources*.

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
  - `deny`: `cargo deny check advisories bans licenses sources`, with
    the policy in `deny.toml`.
- `deny.toml`: yanked crates fail, a `*` version fails, two versions of
  one crate warn, and a crate must come from crates.io or a GitHub
  repository of `DHHwasRight` (brocanth, `rsx`, `jt`), and every crate
  declares a license on the allow-list.

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

**Licenses are checked, except the project's own.** Every crate in the
tree must declare a license on the allow-list in `deny.toml`: Apache-2.0,
the BSD licenses, BSL-1.0, ISC, MIT, PostgreSQL, Unicode-3.0, Zlib and
CDLA-Permissive-2.0.
brocanth's crates, `zod`, `zod-derive`, `rsx` and `jt` declare MIT. The
generated project declares no license: it is an application, not a
library. Its `Cargo.toml` says `publish = false`, and
`[licenses.private] ignore = true` tells `cargo deny` to skip an
unpublished crate. A project made before this change has no
`publish = false`: add it under `[package]`, or the check fails on the
project's own crate.

A new dependency with a license outside the list fails the check. Add its
SPDX id only when the license is permissive. Otherwise add an
`exceptions` entry for that one crate, with the reason, or drop the
crate.

The decisions, from a run of `cargo deny check licenses` (0.20.2) on the
`everything` shape and on the brocanth workspace with every feature:

- `BSL-1.0` (Boost) is on the list: it is permissive. `xxhash-rust`,
  under the Redis client, uses it.
- `PostgreSQL` is on the list: it is permissive, like BSD.
  `postgresql_embedded` and its crates use it.
- `cssparser` and `dtoa-short` are `MPL-2.0`, under `ammonia`, the HTML
  sanitizer. The MPL is a copyleft per file: only a change to one of
  their files must be published. They come unchanged from crates.io, so
  each has an `exceptions` entry. The license is not on the list, so a
  new MPL crate still fails the check and gets its own decision.
- `brocanth-bundle` builds on `lightningcss`, which is `MPL-2.0` with
  four more crates of its tree. No module puts it in a generated project
  today, so `deny.toml` has no entry for them. The module that adds it
  adds their exceptions.

**Why two tools.** `cargo audit` is the RustSec reference and reads
`Cargo.lock` alone. `cargo deny` adds what `cargo audit` does not check:
where a crate comes from, its license, wildcard versions and duplicates. Both read the
same advisory database, so an advisory you accept goes in `ignore` in
`deny.toml` and in `cargo audit --ignore`.

## Settings

None. The policy is `deny.toml`, and the schedule is in the workflow.

## Read more

- [`ci-github`](ci-github.md), which this module needs
- Source: [`crates/brocanth-cli/src/modules/ci.rs`](../../crates/brocanth-cli/src/modules/ci.rs)
