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
  `i18n` plugin, and `.keep_unprefixed(..)` with the path of each module
  that owns a route that is not a page: `admin`'s `mount_at`,
  `storage-routes`' `path`
- config/<env>.toml: `unprefixed` in `[i18n]`
- src/pages/mod.rs: `ctx.href(path)` and the `locale_paths` tests
- AGENTS.md: `## One URL per locale`

The `i18n` hook does the work. It reads the path's first segment, takes it
off, and sets the locale, so `src/pages/about.rs` answers `/fr/about` and
`ctx.path` is `/about`. A GET to a path without a segment gets a 307 to
the locale the visitor asks for. A path under an `unprefixed` prefix, and
a path whose last segment has a `.` (`/robots.txt`), keep no segment.
A WebSocket upgrade and a request that accepts `text/event-stream` are
never redirected, wherever their route is: a socket does not follow a
redirect, and a stream has no locale. So the realtime routes need no
entry.

**Decision: a route another module owns is read from its module file.**
The registration calls `.keep_unprefixed(crate::modules::storage_routes::MODULE.path)`
and the same for `admin`'s `mount_at`. A change in
`src/modules/storage_routes.rs` reaches the hook at the next build, and a
config file cannot remove the entry. The `unprefixed` setting holds only
the project's own prefixes. See [module-config.md](../module-config.md)
§9 for the rule and the alternatives.

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
| `unprefixed` | list | `["/api"]` | none (a list) | Path prefixes that keep no locale segment and are never redirected. The routes of other modules are added in code, after this list. |

## Read more

- [i18n](../i18n.md)
- [`brocanth-i18n`](../../crates/brocanth-i18n/README.md)
- Source: [`crates/brocanth-i18n/src/scaffold.rs`](../../crates/brocanth-i18n/src/scaffold.rs)
