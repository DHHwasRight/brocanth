# `image`

thumbnails converted on request (&fmt=webp), a pixel cap against decompression bombs, a srcset helper.

The wizard calls it: *Images: thumbnails in WebP or JPEG, a srcset helper*.

| | |
|---|---|
| Categories | **Renderer** |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none of its own: `brocanth-storage` (feature `variants`) comes with [`storage-thumbnails`](storage-thumbnails.md) |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/image.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`storage-thumbnails`](storage-thumbnails.md): it converts the thumbnails `storage-thumbnails` renders; without them there is no image to convert |
| After | [`storage-thumbnails`](storage-thumbnails.md), [`storage-routes`](storage-routes.md) |
| Provides | none |
| Requires | none |
| Uses | `ObjectStore` (an object store): the test that stores a PNG runs on `storage-fs` only: Postgres, S3 and GCS are services the tests do not have |
| Adapts to | none |
| Contributes | `AgentsSection` `image`: how a page links an image in a size and a format |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it does

[`storage-thumbnails`](storage-thumbnails.md) already resizes and crops a
stored image behind `?thumb=`, in the sizes `thumb_sizes` lists, renders
each size once and caches it, and reads the image header before it
decodes. This module adds three things on top:

- **Format conversion.** `&fmt=webp` (or `jpeg`, `png`) beside `&thumb=`
  converts the thumbnail. Only a format `formats` lists is served; any
  other is a `400` before the file is read. Each format is cached apart
  (`300x200t.webp` beside `300x200t`) and has its own entity tag.
- **A lower pixel cap.** `max_pixels` (24 million by default) refuses a
  source that declares more pixels with a `413`. The check reads the
  header, so a small file that declares 40000x40000 costs a few bytes of
  reading, and the decoder never allocates for it. The crate's hard cap
  (50 million, `MAX_SOURCE_PIXELS`) and the decoder's 256 MiB budget stay
  behind it.
- **Link helpers.** `src/plugins/image/mod.rs` has `src(ctx, &key, size,
  format)` and `srcset(ctx, &key, sizes, format)`. Each link carries a new
  file token. A `0xH` size has no width, so `srcset` leaves it out. With
  a named store ([`storage`](storage.md#named-stores)), `src_in(ctx,
  "<name>", &key, size, format)` links a thumbnail in that store, at the
  path its routes answer at (`[storage_routes.<name>] path`, read at run
  time), with a token of that store.

## Decisions

- **The `image` crate, already linked.** `storage-thumbnails` uses `image`
  0.25 (MIT or Apache-2.0, maintained by the image-rs organisation) with
  four decoders and no rayon. Its PNG, JPEG and WebP encoders come with
  those decoders, so this module adds no crate and no C library. libvips
  or `sharp`-style bindings would be faster, at the cost of a native
  dependency in every build.
- **WebP is lossless.** The pure-Rust WebP encoder in `image` writes
  lossless WebP only. That suits a logo or a screenshot. A photograph is
  smaller as JPEG, so `jpeg` is in the default list too. AVIF needs an
  encoder (`ravif`/`rav1e`) that is large and slow to build; it is not
  offered.
- **The URL names the format, not `Accept`.** One URL is one image, so a
  CDN caches it without `Vary: Accept`, and a page decides with
  `<picture>` or `srcset` which format to ask for.
- **Allow-lists, not parameters.** Sizes and formats are lists in the
  settings. A free `?w=` would let any token holder make the server render
  and store a thousand sizes of one file.

## What it writes

Files:

- `src/plugins/image/mod.rs`: `src` and `srcset`, and tests that store a
  PNG, ask for a WebP thumbnail through the application, and check that a
  format or a size outside the lists is a `400`.
- `src/modules/image.rs`: no options.

In `storage-routes`, which adapts to it: the `formats` and `max_pixels`
settings in `src/plugins/storage_routes/mod.rs` and `[storage_routes]`.

Slots:

- src/plugins/mod.rs: `pub mod image;`
- AGENTS.md: `## Images`

## Settings

Under `[storage_routes]`, written by [`storage-routes`](storage-routes.md)
when this module is chosen:

| Setting | Default | Meaning |
|---|---|---|
| `formats` | `["webp", "jpeg"]` | The formats `&fmt=` may convert a thumbnail to. Each one is one more rendering per size and file. |
| `max_pixels` | `24000000` | The most pixels a source may declare before a thumbnail of it is a `413`. At most 50000000. |

## Read more

- [`storage-thumbnails`](storage-thumbnails.md), [`storage-routes`](storage-routes.md)
- Source: [`crates/brocanth-storage/src/scaffold.rs`](../../crates/brocanth-storage/src/scaffold.rs), [`crates/brocanth-storage/src/variants.rs`](../../crates/brocanth-storage/src/variants.rs)
