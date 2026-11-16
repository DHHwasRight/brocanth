# `meta`

A title, a description, a canonical URL and Open Graph tags for each page.

The wizard calls it: *A title, a description and share tags for each page*.

| | |
|---|---|
| Categories | **Renderer**, Enhancer |
| Wizard question | Pages |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | none |
| Outside the runtime | yes: the static export renders every page through the layouts |
| Crates | `brocanth-meta` |
| Without it | every page has the title `pages::TITLE`, and no description, canonical URL or share tags |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/meta.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`i18n`](i18n.md): `<html lang>` and `og:locale` follow the request's locale; [`i18n-routing`](i18n-routing.md): a page lists its other locales with hreflang, and its canonical URL keeps the locale segment; [`rsx`](rsx.md): a test reads the tags the root layout writes on the home page; [`sitemap`](sitemap.md): an unset `base_url` is `[sitemap] base_url`, so the two never disagree |
| Contributes | `AgentsSection` `meta`: how a page sets its title and metadata |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[meta]`: `name`, `title_template`, `base_url`, `description`, `image`, `twitter` |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/plugins/meta/mod.rs`: the `[meta]` settings and the `Meta` plugin,
  which puts a `brocanth_meta::Site` on the context. Its tests check the
  title template and the refusal of a template with no `%s`. With
  [`rsx`](rsx.md), a test reads the tags on the home page through
  `app::handle`.

Slots:

- src/plugins/mod.rs: `pub mod meta;`
- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `meta crate::plugins::meta::Meta`
- src/pages/mod.rs: `ctx.meta()` and `ctx.head()`
- config/<env>.toml: `[meta]`
- AGENTS.md: `## Page metadata`

A page sets what differs before it renders:
`ctx.meta().title("About").description("Who writes this.")`. The rest
comes from `[meta]`. `ctx.head()` resolves the two into a
`brocanth_meta::Head`, whose `Display` is the tags, every value escaped.

With [`rsx`](rsx.md), the root layout declares `meta: &Head` and writes
`<title>{meta.title()}</title>` and `{! meta }`. The `rsx` module writes
that layout, because one file has one writer.

With [`i18n`](i18n.md), `<html lang>` and `og:locale` follow the request's
locale. With [`i18n-routing`](i18n-routing.md), the canonical URL keeps the
locale segment and each other locale is an `hreflang` alternate, with an
`x-default`.

## Settings

Section `[meta]`. Environment variable: `$<APP>_META__<KEY>`. Flag: `--meta.<key>`. Rename or hide a flag in `src/modules/meta.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `name` | text | the project's title | `--meta.name` | The site's name: the title of a page with none, and `og:site_name`. |
| `title_template` | text, optional | `"%s \| <title>"` (commented) | `--meta.title_template` | How a page's title becomes the document's. `%s` is the page's own title. |
| `base_url` | text, optional | `"https://example.com"` (commented) | `--meta.base_url` | The site's address, scheme and host. Unset, a page has no canonical URL. |
| `description` | text, optional | `""` (commented) | `--meta.description` | The description of a page that gives none. |
| `image` | text, optional | `"/og.png"` (commented) | `--meta.image` | The `og:image` of a page that gives none: a path or a full URL. |
| `twitter` | text, optional | `"@example"` (commented) | `--meta.twitter` | The site's account, for `twitter:site`. |

A template with no `%s` or a base URL with no scheme stops the start with
a `help:` line: every page would carry the wrong tag.

**Decision: no canonical URL without `base_url`.** A relative canonical URL
is worse than none: a crawler resolves it against whatever host served the
page, a staging host too.

## Read more

- [`brocanth-meta`](../../crates/brocanth-meta/README.md)
- Source: [`crates/brocanth-meta/src/scaffold.rs`](../../crates/brocanth-meta/src/scaffold.rs)
