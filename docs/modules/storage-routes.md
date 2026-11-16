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
| Needs | [`middleware-csrf`](middleware-csrf.md): an upload is a form post, and the routes read the check's field and header |
| After | [`storage`](storage.md), [`config`](config.md) |
| Adapts to | [`storage-thumbnails`](storage-thumbnails.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-storage` (feature `routes`; `variants` with [`storage-thumbnails`](storage-thumbnails.md)) |
| Without it | no upload or download route; a handler stores and serves each file itself |

## What it does

- `POST /files` takes a `multipart/form-data` form and stores its `file`
  part. It answers `303` to the form's `return_to` field with `?file=` and
  `&token=` added, or `201` with the address when there is no `return_to`.
- `GET /files/{address}?token=` serves the file. The token comes from
  `FileStore::tokens`, which your page calls after your policy allowed the
  read. Without a valid token the answer is `403`.
- With [`storage-thumbnails`](storage-thumbnails.md), `&thumb=WxH` serves a
  thumbnail in a size that `thumb_sizes` lists. Any other size is a `400`.
- A file that is not a known image is served as an attachment of type
  `application/octet-stream`, never inline. The type the client declared
  is not repeated.

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

## Read more

- [`storage`](storage.md)
- [`storage-thumbnails`](storage-thumbnails.md)
- [brocanth-storage](../../crates/brocanth-storage/README.md)
- Source: [`crates/brocanth-storage/src/scaffold.rs`](../../crates/brocanth-storage/src/scaffold.rs), [`crates/brocanth-storage/src/routes.rs`](../../crates/brocanth-storage/src/routes.rs)
