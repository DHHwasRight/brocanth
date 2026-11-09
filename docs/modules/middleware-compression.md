# `middleware-compression`

gzip and brotli per Accept-Encoding on text responses, with Vary; never a stream.

The wizard calls it: *Compress pages for the browsers that ask*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`middleware`](middleware.md), off until you tick it |
| Needs | none |
| After | [`middleware-conditional-get`](middleware-conditional-get.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-middleware` (features `compression`) |
| Without it | no compression; an HTML page goes over the wire five to ten times its gzip size |

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `middleware-compression brocanth_middleware::Compression`
- src/plugins/mod.rs (the hook record): `brocanth_middleware::Compression`
- config/<env>.toml: `[compression]`
- AGENTS.md: `## Compression`

With `config`, the registration reads `[compression]` through `plugin_hooking_from_config`. Without `config`, it starts from `CompressionConfig::default()` and applies the environment and the flags. A list has no flag, so `types` needs `config` or code.

The `compression` feature of `brocanth-middleware` links `flate2` and `brotli`, both pure Rust.

## What the hook does

It reads `Accept-Encoding` (q-values, `*`, `x-gzip`), picks `br` or `gzip`, and rewrites a buffered body. On a tie, `br` wins: it is smaller. If the result is not smaller, the identity body goes out.

| Left alone | Why |
|---|---|
| a stream: SSE, a download, a `101` | an SSE event must not wait for a compressor's window |
| `Content-Encoding` already set | the page compressed it, or it is a precompressed asset |
| status `1xx`, `204`, `205`, `206`, `304` | no body, or a byte range |
| `Cache-Control: no-transform` | the page forbids it |
| a media type off `types`, or `text/event-stream` | an image or a font is already compressed |
| a body under `min_bytes` | the frame costs more than it saves |

Every response that could be compressed gets `Vary: Accept-Encoding`, also for a client that asked for none, so a shared cache keys on it. A strong `ETag` on a compressed body becomes weak.

**Order.** The hook runs last: after `conditional_get`, `route_cache`, `gtag` and `beacon`, which read or rewrite the body. Its `Order` names them. Add a hook of your own to `runs_after` (in the file with `config`, or in code).

**Behind a CDN.** Most CDNs and proxies compress. Then leave this off: the second pass costs CPU and gains nothing.

## The module file

`src/modules/middleware_compression.rs` has no hook switch. The module is one hook, `encode`, and it is required: a project behind a CDN that compresses removes the module, and one that wants only gzip sets `brotli = false`.

## Settings

Section `[compression]`, the plugin's instance name, not the module key. Environment variable: `$<APP>_COMPRESSION__<KEY>`. Flag: `--compression.<key>`. Rename or hide a flag in `src/modules/middleware_compression.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `min_bytes` | integer | `1024` | `--compression.min_bytes` | The smallest body that is compressed, in bytes. |
| `gzip` | bool | `true` | `--compression.gzip` | Offer gzip. |
| `brotli` | bool | `true` | `--compression.brotli` | Offer brotli (`br`). |
| `gzip_level` | integer | `6` | `--compression.gzip_level` | The gzip level, 1 (fast) to 9 (small). |
| `brotli_quality` | integer | `4` | `--compression.brotli_quality` | The brotli quality, 0 (fast) to 11 (small). 4 or 5 per request; 11 is for files compressed once at build time. |
| `types` | list | `["text/*", "application/json", ...]` | none (a list) | The media types that are compressed. `text/*` takes every text type. |

A level out of range, both codings off, or a `types` entry that is not `type/subtype` stops the start with a `help:` line.

## Read more

- [Middleware](../middleware.md) §8
- Source: [`crates/brocanth-middleware/src/compression.rs`](../../crates/brocanth-middleware/src/compression.rs), [`crates/brocanth-middleware/src/scaffold.rs`](../../crates/brocanth-middleware/src/scaffold.rs)
