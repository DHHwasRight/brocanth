# `storage-thumbnails`

`?thumb=WxH` on the file routes, in listed sizes, rendered once each; links four image decoders.

The wizard shows it under `storage`, in the Data question.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | [`storage`](storage.md), off until you tick it |
| Needs | [`storage-routes`](storage-routes.md): the thumbnails answer `?thumb=` on the file routes |
| After | [`storage-routes`](storage-routes.md) |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-storage` (feature `variants`) |
| Without it | the file routes answer no `?thumb=`, and the image decoders are not linked |

## What it does

The module writes no file. It turns on the `variants` feature of
`brocanth-storage`, which links the image decoders. Then `storage-routes`
answers `&thumb=WxH` and adds the `thumb_sizes` setting to
`[storage_routes]`.

**Only the listed sizes.** A size that `thumb_sizes` does not list gets a
`400` before the store is read or an image is decoded. Reason: each new size
is a decode and a stored file, so an open list is a free disk and a free CPU
for every token holder.

**A feature, not an option.** An option is read at run time. The decoders
are linked at build time, so only a feature keeps them out of a project that
shows no thumbnail.

Each size is rendered once per file and kept. A render runs on the blocking
pool, at most four at a time.

## Settings

It adds one key to `[storage_routes]`:

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `thumb_sizes` | list | `["150x150t", "300x200", "600x600f"]` | `--storage_routes.thumb_sizes` | The only sizes `?thumb=` may ask for; any other is a 400. |

`WxH` crops to the middle, `WxHt` to the top, `WxHb` to the bottom, `WxHf`
fits, `Wx0` and `0xH` scale.

## Read more

- [`storage-routes`](storage-routes.md)
- [brocanth-storage](../../crates/brocanth-storage/README.md)
- Source: [`crates/brocanth-storage/src/scaffold.rs`](../../crates/brocanth-storage/src/scaffold.rs), [`crates/brocanth-storage/src/variants.rs`](../../crates/brocanth-storage/src/variants.rs)
