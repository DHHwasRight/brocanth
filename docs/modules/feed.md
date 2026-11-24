# `feed`

An Atom feed of the posts at /feed.xml, with absolute URLs from `--feed.site_url`.

The wizard calls it: *An Atom feed of the posts*.

| | |
|---|---|
| Categories | **Routes** |
| Wizard question | Pages |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | none |
| Outside the runtime | yes: the static export writes `/feed.xml` as the file `feed.xml` |
| Crates | `brocanth-content` |
| Without it | no feed; a reader learns of a new post by visiting the site |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/feed.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`content`](content.md): the feed lists the posts of the collection |
| After | [`content`](content.md) |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`static`](static.md): the development file leaves `site_url` empty: the export is the published site; [`i18n-routing`](i18n-routing.md): the test reads the layout's link on the home page under /en |
| Contributes | `AgentsSection` `feed`: what the feed needs before it is published |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[feed]`: `site_url`, `title`, `author`, `limit` |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/pages/feed.xml.rs`: `/feed.xml`, `application/atom+xml`, an hour of
  cache. With no `site_url`, a 500 with the fix in the log.
- `src/plugins/feed/mod.rs`: the `[feed]` settings, `render(&Settings)`, and
  the `Feed` plugin, which renders the feed once at `configure`. Its tests
  render with a site URL, check the refusal without one, and read the
  layout's link on the home page through `app::handle`.

Slots:

- src/plugins/mod.rs: `pub mod feed;`
- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `feed crate::plugins::feed::Feed`
- src/pages/mod.rs (the route table test): `/feed.xml`
- config/<env>.toml: `[feed]`
- AGENTS.md: `## Feed`

With [`rsx`](rsx.md) (always: `content` needs it), the root layout writes
`<link rel="alternate" type="application/atom+xml" title={TITLE}
href="/feed.xml">` in `<head>`. The `rsx` module writes that line, because one
file has one writer.

## Settings

Section `[feed]`. Environment variable: `$<APP>_FEED__<KEY>`. Flag: `--feed.<key>`. Rename or hide a flag in `src/modules/feed.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `site_url` | text, required | `"http://localhost:3000"` (`""` with `static`) | `--feed.site_url` | The site's address, scheme and host: every URL in the feed starts with it. |
| `title` | text | the project's title | `--feed.title` | The feed's title. |
| `author` | text | the project's title | `--feed.author` | The author Atom names for the feed. |
| `limit` | integer | `20` | `--feed.limit` | The most posts the feed lists, newest first. |

The plugin's own default for `site_url` is empty: a project without a config
file passes `--feed.site_url=` or sets `$<APP>_FEED__SITE_URL`.

## Decisions

**Atom, not RSS 2.0.** Atom is an IETF standard (RFC 4287): each entry has a
required id and `updated` date, a date has one format (RFC 3339), and HTML
content says it is HTML. RSS 2.0 leaves each of these to the reader. Every
feed reader in use reads Atom, so one format is one URL to announce.

**`site_url` is required, and a missing one stops the start.** A feed is read
away from the site, so every URL in it is absolute; a feed of `localhost` URLs
sends every reader nowhere. Unset or empty, the plugin registers a readiness
check that fails, so `serve` stops before it binds with `[feed] ...` and a
`help:` line, and `/readyz` answers 503. A project's own tests build the
application and serve nothing, so they run without one. The static export
stops on the 500 of `/feed.xml`.

**On a static site, the development file leaves it empty.** The export is the
published site; a development address there would be published. The note in
the file says to set the published address.

**Its own setting.** `[sitemap] base_url` and `[meta] base_url` hold the same
address. `feed` needs neither module, and `--feed.site_url` is the flag the
module promises. A framework-level `[app] url` would hold it once; the
framework has no such setting yet.

**Published posts only.** No draft, whatever `[content] drafts` says, and no
translation: a feed is one language. A post with no date is left out, because
Atom needs one.

**Relative links in a post resolve.** Each entry's content carries `xml:base`,
the post's absolute URL, so `/about` in a post points at the site.

## Read more

- [`brocanth-content`](../../crates/brocanth-content/README.md)
- Source: [`crates/brocanth-content/src/feed.rs`](../../crates/brocanth-content/src/feed.rs), [`crates/brocanth-content/src/scaffold_feed.rs`](../../crates/brocanth-content/src/scaffold_feed.rs)
