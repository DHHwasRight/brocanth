# `content`

Markdown files with typed front matter, checked by the build, listed at /posts; no database.

The wizard calls it: *A blog: Markdown posts with typed front matter*.

| | |
|---|---|
| Categories | **Enhancer**, Routes |
| Wizard question | Pages |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | none |
| Outside the runtime | yes: the static export writes the list, each published post and each tag page |
| Crates | `brocanth-content`, and `brocanth-content` (feature `build`) and `serde` in `[build-dependencies]` |
| Without it | no content collections; a post is a page written by hand, with no checked front matter |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/content.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`markdown`](markdown.md): each entry's body is rendered and cleaned by it; [`rsx`](rsx.md): the list and the post are `.rsx` templates |
| After | [`markdown`](markdown.md), [`rsx`](rsx.md), [`i18n`](i18n.md) |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`meta`](meta.md): a post's title and description are the page's title, description and share tags; [`i18n`](i18n.md): `hello.fr.md` translates `hello.md`, and a page lists the request's locale; [`i18n-routing`](i18n-routing.md): the links keep the locale segment, and the tests ask under /en; [`static`](static.md): drafts are hidden unless asked for, because the export is what gets published |
| Contributes | `AgentsSection` `content`: how a post is written and how the collection grows |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[content]`: `drafts` |
<!-- end generated: capabilities -->

## What it writes

Files:

- `content/posts/hello.md`: the first post. With [`i18n`](i18n.md), also
  `content/posts/hello.fr.md`, its translation.
- `src/plugins/content/posts.rs`: the front matter as a Rust type, `Post`
  (`title`, `description`, `date`, `updated`, `tags`, `draft`), with
  `deny_unknown_fields`. The project owns it and adds fields.
- `src/plugins/content/mod.rs`: the files the build step listed, embedded
  with `include_str!`; `collection()` and `paths()`; the `Content` plugin,
  which reads the collection at `configure` and puts a `Posts` view on each
  request's context. Its tests read every post.
- `src/pages/posts/index.rs` and `index.rsx`: `/posts`, newest first.
- `src/pages/posts/[slug].rs` and `[slug].rsx`: `/posts/{slug}`, one post;
  a slug with no file, or a hidden draft, is a 404. Each tag links to its
  page.
- `src/pages/posts/tags/index.rs` and `index.rsx`: `/posts/tags`, every
  tag of the listed posts with how many carry it, most used first.
- `src/pages/posts/tags/[tag].rs` and `[tag].rsx`: `/posts/tags/{tag}`,
  the posts with that tag, newest first, in the request's locale; a tag no
  listed post carries is a 404.

Slots:

- build.rs (main): the collection step
- src/plugins/mod.rs: `pub mod content;`
- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `content crate::plugins::content::Content`
- src/pages/mod.rs (the route table test): `/posts`, `/posts/tags`,
  `/posts/tags/{tag}`, `/posts/{slug}`
- config/<env>.toml: `[content]`
- AGENTS.md: `## Content`

The page tests ask for `/posts`, `/posts/hello`, `/posts/tags` and
`/posts/tags/meta` through `app::handle` and check the links, the date, the
tag count, the rendered heading and table, and the 404s. With
[`meta`](meta.md), the post's `<title>` too; with `i18n`, the French list
and the French tag page.

## Settings

Section `[content]`. Environment variable: `$<APP>_CONTENT__<KEY>`. Flag: `--content.<key>`. Rename or hide a flag in `src/modules/content.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `drafts` | bool | commented out | `--content.drafts` | Show drafts in the list and at their URL. |

Unset, drafts are shown in a debug build (`cargo run`) and hidden in a release
build. With [`static`](static.md), unset is hidden. A draft is never in
`paths()`, so never in the static export, and never in the
[`feed`](feed.md). The tag pages follow the same rule: a draft's tags count
and list only where drafts are shown.

`paths()` is the index, each published post, `/posts/tags` and each tag
page, so the static export writes every tag page.
`Collection::dated_paths()` gives the same paths with the day each last
changed; the [`sitemap`](sitemap.md) reads it.

## Decisions

**The build checks, the binary renders.** `build.rs` reads every
`content/posts/*.md` into `Post` and stops the build on a bad file: a name
that is not a slug, front matter that does not read (a missing field, a value
of the wrong type, an unknown key), an empty title, or a translation with no
default file. Each error names the file and its line and ends with a `help:`
line, and every bad file is listed, not only the first. A typo is a mistake in
the source, so it never reaches a server or a published export. Contentlayer
and Astro collections check at build time for the same reason.

**One type, compiled twice.** `build.rs` takes `src/plugins/content/posts.rs`
through a `#[path]` module, so the check and the binary read one
definition. The cost: that file may use `serde`, `std` and `brocanth_content`
only.

**The build script links no parser.** It writes a list of files to
`$OUT_DIR/content_posts.rs`; the binary embeds them and renders each body
once, at start, through [`markdown`](markdown.md). `brocanth-content`'s
`build` feature takes only the front-matter half of `brocanth-markdown`. A few
hundred posts render in milliseconds.

**A new file rebuilds.** The step prints `cargo:rerun-if-changed` for the
directory and for each file, so adding, editing or deleting a post rebuilds.
(`pages!` and `rsx::load_dir!` cannot see a new file; a build script can.)

**One `build.rs` for every module.** `i18n` compiled its catalogs in a
`build.rs` it wrote whole, so `content` beside it would clash. The generator
now writes `build.rs` from `Slot::BuildMain`, one block per module, and
rewrites it on `add` and `remove`.

**No database.** The posts are in the binary. A post is edited in git and
deployed, as code is.

**`rsx` is required.** The pages are templates. A list built as a
`String` is code an `rsx` project never writes.

**One collection.** A second one copies the pattern: a directory, a type, a
step in `build.rs`, a plugin and four pages. AGENTS.md says so. A generator for
collections waits for the second request.

**A translation is a file.** `hello.fr.md` beside `hello.md`. A page lists
the request's locale and falls back to the default file. With
[`i18n-routing`](i18n-routing.md), the links keep the locale segment. A
translation with no default file fails the build: the list, the feed and every
other locale fall back to it.

**A tag is a page.** `/posts/tags` lists the tags, `/posts/tags/{tag}` the
posts of one. They read `View::tags()` and `View::tagged(tag, locale)`, so
the drafts rule is the list's.

**A tag is a slug.** `build.rs` refuses a tag that is not lower case
letters, digits and single `-`, with the file and a `help:` line. The tag
is the last URL segment of its page and a file name in the static export,
so `Web Dev` or `C++` would need an encoding and a second spelling.
Mapping a tag to a slug (`Web Dev` to `web-dev`) was rejected: two tags
would share one page, and the page would need the written form back.

**A post cannot be `tags.md`.** The router prefers the literal segment
`tags` to `{slug}`, so that post would never be served. `build.rs` refuses
the slug (`brocanth_content::build::RESERVED`).

**The default file decides a post's tags.** A tag belongs to the post, not
to a translation: the tag page lists the same posts in every locale, each
in its translation where there is one, and the post page links the default
file's tags.

## Read more

- [`brocanth-content`](../../crates/brocanth-content/README.md)
- Source: [`crates/brocanth-content/src/scaffold.rs`](../../crates/brocanth-content/src/scaffold.rs), [`crates/brocanth-content/src/build.rs`](../../crates/brocanth-content/src/build.rs), [`crates/brocanth-content/src/collection.rs`](../../crates/brocanth-content/src/collection.rs)
