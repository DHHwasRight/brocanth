# brocanth

A server framework for HTML-first applications in Rust. Cargo workspace of
about 33 crates under `crates/`, two examples under `examples/`.

## Read first

- `PLAN.md` — the design. §12 has the build order. §13 has the ADRs.
- `docs/framework-gaps.md` — ranked gaps (G1–G14) with a status table.
  Update the table when you close a gap.
- `docs/llm-authoring.md` — the contract for writing brocanth with a model.
- `docs/modules.md` — how a crate reaches a generated project (`Slot`s,
  `scaffold` feature, the CLI registry).
- `crates/brocanth-queue/docs/DECISIONS.md` — the queue design and its open
  items (Part 4).

## Commands

Toolchain is pinned by `mise.toml` (Rust 1.98). Run through mise:

```
mise run build   # cargo build --workspace
mise run test    # cargo test --workspace
mise run lint    # cargo clippy --all-targets -- -D warnings
mise run ci      # all three
```

Also run `cargo fmt --check` before a commit.

## Tests

- Postgres tests are skipped when no database is set. Set
  `BROCANTH_TEST_DATABASE_URL` to run them all. Per-crate overrides exist:
  `BROCANTH_QUEUE_TEST_DATABASE_URL`, `BROCANTH_MODEL_TEST_POSTGRES`,
  `BROCANTH_JOBS_TEST_POSTGRES`, `BROCANTH_CACHE_TEST_POSTGRES`.
  `BROCANTH_QUEUE_REQUIRE_POSTGRES=1` makes a missing database a failure.
- A Postgres test creates its own schema. Never truncate a shared table.
- CLI golden files live in `crates/brocanth-cli/tests/golden/`. When the
  generator changes, run
  `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- generator_writes`
  and read the diff before you commit it.
- Use `brocanth-test` (`TestClient`, HTML reader) to test a screen. Assert on
  behaviour, not on CSS class names.

## Templates and editors

- `.rsx` files are templates for the `rsx` compiler
  (github.com/DHHwasRight/rsx). `.jt` files are `jt` templates
  (github.com/DHHwasRight/jt). Both have language servers. The LSP tool works
  on them: hover, definition, references, workspace symbols.
- Both servers are installed binaries. After you change `rsx-lsp` or
  `jt-lsp`, reinstall: `cargo install --path crates/rsx-cli --force` in
  `rust/templ`, `cargo install --path crates/jt-lsp --force` in `rust/jt`.

## Conventions

- A capability crate reaches applications through a `Module` behind an
  off-by-default `scaffold` feature. Nothing ships unless it is asked for
  (PLAN.md §8).
- Authorization goes through `brocanth-policy`: `condition()` for lists,
  `Verified` consumed by the write. Do not filter after a paginated load.
- Errors and CLI diagnostics carry a `help:` line that says what to do next.
- Commits: conventional (`feat(scope): …`), one subject that says what now
  works. Stage by explicit path.
- Docs: short sentences, active voice. Record a decision with its reason.

## Remote

`git@github.com:DHHwasRight/brocanth.git`, branch `main`.
