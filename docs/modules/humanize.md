# `humanize`

readable values for a page: "3 minutes ago", 1.5 MB, 2nd, 1,234; no dependency.

The wizard calls it: *Readable times, sizes and numbers: "3 minutes ago"*.

| | |
|---|---|
| Categories | **Renderer** |
| Wizard question | Pages |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-humanize` (feature `i18n` with [`i18n`](i18n.md)) |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/humanize.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`i18n`](i18n.md): the phrases follow the request's locale, and numbers group by its CLDR formats |
| Contributes | `AgentsSection` `humanize`: how a page writes a time, a size or a count for a reader |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | `locale` = `"en"`: The language of the phrases without the i18n module: en, fr, de or es. |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it does

A page asks `crate::plugins::humanize::of(ctx)` for the helpers in the
request's language:

| Call | English | French |
|---|---|---|
| `ago(180)` | 3 minutes ago | il y a 3 minutes |
| `ago(-7200)` | in 2 hours | dans 2 heures |
| `since(then, now)` | the same, from two `SystemTime`s | |
| `file_size(1_500_000)` | 1.5 MB | 1,5 Mo |
| `ordinal(1)` | 1st | 1er |
| `number(1234567)` | 1,234,567 | 1 234 567 |

English, French, German and Spanish have their own phrases. Any other
language gets English phrases, never a key or an empty string.

**With [`i18n`](i18n.md)**, the language is the locale the `i18n` hook
chose, and `number` groups by that locale's CLDR formats, so a page and its
catalog write a number alike. **Without it**, the language is the `locale`
option in `src/modules/humanize.rs`.

## Decisions

- **Sizes are decimal.** 1 kB is 1000 bytes, as disks, networks and macOS
  count. A site that shows an upload limit and a file size must use one
  unit, and the reader's other tools show SI. There is no `KiB` mode: two
  units on one site is the bug.
- **A time is a whole unit, rounded down.** 90 seconds is "1 minute ago". A
  reader who sees "2 minutes" expects at least two. Under a minute is "just
  now": a count of seconds is stale before the page is read.
- **A month is 30 days, a year 365.** The phrase is approximate on
  purpose. Put the exact value in `<time datetime>` beside it.
- **No plugin.** The helpers hold no state and read nothing at start, so a
  registration would do nothing.

## What it writes

Files:

- `src/plugins/humanize/mod.rs`: `of(ctx)`, and tests that check the
  phrases a page gets.
- `src/modules/humanize.rs`: the `locale` option.

Slots:

- src/plugins/mod.rs: `pub mod humanize;`
- AGENTS.md: `## Readable values`

## Settings

None at run time. `locale` is an option, set at build time: the language
is a fact about the application, the same in every environment.

## Read more

- [`i18n`](i18n.md)
- Source: [`crates/brocanth-humanize/src/scaffold.rs`](../../crates/brocanth-humanize/src/scaffold.rs)
