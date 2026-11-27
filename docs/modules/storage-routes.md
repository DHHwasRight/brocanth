# `storage-routes`

The HTTP edge of `storage`: an upload route your guard allows, a download route behind a file token.

The wizard calls it: *Upload and download routes, behind a guard and file tokens*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | [`storage`](storage.md), off until you tick it |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-storage` (feature `routes`; `variants` with [`storage-thumbnails`](storage-thumbnails.md)) |
| Without it | no upload or download route; a handler stores and serves each file itself |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/storage_routes.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`middleware-csrf`](middleware-csrf.md): an upload is a form post: without the CSRF check any site posts files here with the visitor's cookies, and the routes read its field and header |
| After | [`middleware-csrf`](middleware-csrf.md), [`storage`](storage.md) |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`storage-thumbnails`](storage-thumbnails.md): `?thumb=WxH` answers in the sizes `thumb_sizes` lists; [`image`](image.md): `&fmt=` converts a thumbnail to a format `formats` lists, and `max_pixels` caps the source; [`storage-s3`](storage-s3.md): `downloads` and `signed_ttl`: a download may redirect to a signed URL of the bucket; [`storage-gcs`](storage-gcs.md): `downloads` and `signed_ttl`: a download may redirect to a signed URL of the bucket |
| Contributes | `AgentsSection` `storage-routes`: which file routes exist, and how a file is served |
| Collects | none |
| Exports | none |
| Hooks | `answer` (Cache, required): answer POST {path} and GET {path}/{address}; `lift` (Guard, on by default): copy an upload form's CSRF field into the header the CSRF check reads |
| Options (build time) | `path` = `"/files"`, mount, Machine: Where the routes answer: uploads at {path}, files at {path}/{address}.; `public` = `false`: Serve a file to a request with no token. Off, every download needs a token. |
| Settings (run time) | `[storage_routes]`: `max_bytes`, `dedupe` |
<!-- end generated: capabilities -->

## Named stores

Each named store ([`storage`](storage.md#named-stores)) gets its own routes,
at `path/<name>`: uploads at `/files/public`, files at
`/files/public/{address}`. An address is 64 hex characters, so a store name
never takes its place. The routes of the named stores answer before the
default's: the default routes would answer `POST /files/public` with a 405.

| Per store | Where |
|---|---|
| Who may upload | `may_upload_<name>` in `src/plugins/storage_routes/mod.rs`; it answers no until you change it |
| `path` | `[storage_routes.<name>] path`; empty means `path/<name>`. The name is then part of a public URL: a new name breaks links already sent |
| `public`, `max_bytes` | `[storage_routes.<name>]` |
| `downloads` | `[storage_routes.<name>]`, cloud stores only. `proxy` is the one value this build takes |

The thumbnails, `dedupe` and the formats are the default store's. A
thumbnail lives in the store of its object.

## What it does

- `POST /files` takes a `multipart/form-data` form and stores its `file`
  part. It answers `303` to the form's `return_to` field with `?file=` and
  `&token=` added, or `201` with the address when there is no `return_to`.
- `GET /files/{address}?token=` serves the file. The token comes from
  `FileStore::tokens`, which your page calls after your policy allowed the
  read. Without a valid token the answer is `403`.
- With [`storage-thumbnails`](storage-thumbnails.md), `&thumb=WxH` serves a
  thumbnail in a size that `thumb_sizes` lists. Any other size is a `400`.
- With [`image`](image.md), `&fmt=webp` beside `&thumb=` converts the
  thumbnail to a format `formats` lists, and a source past `max_pixels` is
  a `413`.
- A file that is not a known image is served as an attachment of type
  `application/octet-stream`, never inline. The type the client declared
  is not repeated.
- With [`storage-s3`](storage-s3.md) or [`storage-gcs`](storage-gcs.md),
  `downloads = "redirect"` answers a download with a `307` to a URL the
  bucket signs. See [Redirected downloads](#redirected-downloads).

## Redirected downloads

Off by default: `downloads = "proxy"` sends the bytes from this process.
Opt in per store when the files are large or many. Reason: a redirect needs
a private bucket, a CSP change and a key that can sign, and the proxy needs
none of them (`docs/object-storage.md` §9, D28).

With `downloads = "redirect"`:

- A `GET` with a valid token answers `307` to a URL the store signs for
  `signed_ttl` seconds (default 300, from 1 to 604800). The bucket sends
  the bytes and answers a `Range` itself.
- The URL carries the type and the disposition inside its signature. A
  sniffed PNG, JPEG, GIF or WebP goes inline as its type. Every other file
  goes as an attachment of type `application/octet-stream`, SVG and PDF
  included. A URL with an override removed has no valid signature.
- The `307` holds `Cache-Control: private, max-age=<min(token time left,
  signed_ttl / 2)>` and `Referrer-Policy: no-referrer`. The process keeps
  each signed URL for half its life, so a second page view gets the same
  URL and the browser cache works.
- `HEAD`, `&thumb=` and `&fmt=` keep proxying. A URL signed for `GET`
  refuses `HEAD`, and a thumbnail is small and already cached.
- New uploads of a non-image also store `Content-Disposition: attachment`
  on the object, so any URL that reaches it downloads.

What the bucket cannot send: `X-Content-Type-Options: nosniff` and
`Content-Security-Policy: sandbox`. The forced attachment covers them: a
browser saves an attachment and renders nothing (`docs/object-storage.md`
§9.4). Keep the bucket private: a public bucket serves every object with
its stored headers to anyone who has the address.

The start refuses a mistake before the server listens:

- On a disk or Postgres store, `redirect` fails at start with a `help:`
  line. Such a store cannot sign.
- The start signs one URL. A key that cannot sign fails there.
- With [`middleware-security-headers`](middleware-security-headers.md), a
  policy whose `img-src` does not allow the bucket's origin fails the start,
  and the `help:` line names the value:

  ```toml
  [security_headers.csp.allow]
  img-src = ["https://uploads.s3.eu-west-1.amazonaws.com"]
  ```

`check --deploy` warns about `redirect` with an `http://` endpoint, a
`signed_ttl` above 3600 seconds, and an endpoint on the site's own
registrable domain.

**Who may upload is your decision.** The module writes `may_upload` in
`src/plugins/storage_routes/mod.rs`, and it answers no. It is async, so it
can await a policy check that loads a row. Nobody can upload
until you edit it. Reason: a generated "yes" is a free disk for every
stranger, and the generator cannot know your policy.

**A plain form works with no script.** The module needs `middleware-csrf`,
and reads the check's field and header from the CSRF plugin, so it keeps no
copy of either. The CSRF check reads no multipart body. The `lift` hook runs before the check and copies the form's CSRF
field into the header that the check reads (both in `Phase::Guard`, where the check is last). The `answer` hook runs in `Phase::Cache`, after
every guard, so an upload from another site stops at the check.

## What it writes

Files:

- `src/plugins/storage_routes/mod.rs`: the settings, `may_upload`, and a
  plugin that installs `brocanth_storage::routes::FileRoutes` as two request
  hooks.
- `src/modules/storage_routes.rs`: the choices below.

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `storage-routes crate::plugins::storage_routes::StorageRoutes`
- src/plugins/mod.rs (the hook record): `crate::plugins::storage_routes::StorageRoutes`
- src/plugins/mod.rs: `pub mod storage_routes;`
- config/<env>.toml: `[storage_routes]`
- AGENTS.md: `## File routes`

The `storage` plugin also puts the store and the token service on the
services, where this plugin reads them at its own `configure`.

## Choices

In `src/modules/storage_routes.rs`. A choice is read at build time, not per
deployment.

| Choice | Default | What it does |
|---|---|---|
| hook `answer` | always on | Answers `POST {path}` and `GET {path}/{address}`. The module is this hook. |
| hook `lift` | on | Copies an upload form's CSRF field into the header the CSRF check reads. |
| `path` | `"/files"` | Where the routes answer. |
| `public` | `false` | Serve a file to a request with no token. |

## Settings

Section `[storage_routes]`, the plugin's instance name, not the module key. Environment variable: `$<APP>_STORAGE_ROUTES__<KEY>`. Flag: `--storage_routes.<key>`. Rename or hide a flag in `src/modules/storage_routes.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `max_bytes` | integer | `2097152` | `--storage_routes.max_bytes` | The largest file accepted, in bytes. At most `[server] max_body_bytes` (16 MiB by default): a larger value stops the start, because the server would refuse the upload first. |
| `dedupe` | bool | `false` | `--storage_routes.dedupe` | Store identical uploads once, at their content address. Off, each upload has an address of its own. On, a delete removes the file for every holder: count the references before you delete. |
| `thumb_sizes` | list | `["150x150t", "300x200", "600x600f"]` | `--storage_routes.thumb_sizes` | With [`storage-thumbnails`](storage-thumbnails.md) only. The only sizes `?thumb=` may ask for; any other is a 400. |
| `downloads` | text | `"proxy"` | `--storage_routes.downloads` | With [`storage-s3`](storage-s3.md) or [`storage-gcs`](storage-gcs.md) only. `"proxy"` sends the bytes from this process; `"redirect"` answers 307 to a signed URL of the bucket. |
| `signed_ttl` | integer | `300` | `--storage_routes.signed_ttl` | With a bucket only. Seconds a signed URL of a redirect lives, from 1 to 604800. It cannot be revoked, so this is the leak window. |

## Read more

- [`storage`](storage.md)
- [`storage-thumbnails`](storage-thumbnails.md)
- [brocanth-storage](../../crates/brocanth-storage/README.md)
- Source: [`crates/brocanth-storage/src/scaffold.rs`](../../crates/brocanth-storage/src/scaffold.rs), [`crates/brocanth-storage/src/routes.rs`](../../crates/brocanth-storage/src/routes.rs)
