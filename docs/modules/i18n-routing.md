# `i18n-routing`

A locale segment in the path (/fr/about), a redirect from a path without one.

The wizard calls it: *One URL per language: /fr/about*.

| | |
|---|---|
| Categories | **Middleware**, Renderer |
| Wizard question | Pages |
| Default | off |
| Presets | none |
| Part of | [`i18n`](i18n.md), off until you tick it |
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none: the hook is `brocanth-i18n`'s |
| Without it | the locale comes from the query, a cookie or Accept-Language; a search engine indexes one language |

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain): `.with_paths(true)` on the
  `i18n` plugin
- config/<env>.toml: `unprefixed` in `[i18n]`
- src/pages/mod.rs: `ctx.href(path)` and the `locale_paths` tests
- AGENTS.md: `## One URL per locale`

The `i18n` hook does the work. It reads the path's first segment, takes it
off, and sets the locale, so `src/pages/about.rs` answers `/fr/about` and
`ctx.path` is `/about`. A GET to a path without a segment gets a 307 to
the locale the visitor asks for. A path under an `unprefixed` prefix, and
a path whose last segment has a `.` (`/robots.txt`), keep no segment.

`ctx.href("/about")` keeps the page's locale. With [`meta`](meta.md),
every page lists its other locales with `hreflang`, and its canonical URL
keeps the segment.

**Decision: a child of `i18n`, off by default.** It changes how that
module's hook reads the locale and needs nothing else. A site may want one
URL per page, so ticking `i18n` does not tick it.

**Decision: paths are not a setting.** The module turns them on and
removing the module turns them off, so a config file cannot leave a
project with links that point nowhere.

## Settings

It adds one key to the `[i18n]` section of [`i18n`](i18n.md):

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `unprefixed` | list | `["/admin", "/api"]` | none (a list) | Path prefixes that keep no locale segment and are never redirected. |

## Read more

- [i18n](../i18n.md)
- [`brocanth-i18n`](../../crates/brocanth-i18n/README.md)
- Source: [`crates/brocanth-i18n/src/scaffold.rs`](../../crates/brocanth-i18n/src/scaffold.rs)
