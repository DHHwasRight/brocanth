# `flash`

One-shot messages across a redirect, in a signed cookie, drawn into the layout.

The wizard calls it: *Show "Saved" on the page a form redirects to*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | [`middleware-rate-limit`](middleware-rate-limit.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-middleware` (feature `config` with `config`) |
| Without it | no flash; a page a form redirects to cannot say what the form did |

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `flash brocanth_middleware::FlashPlugin`
- src/plugins/mod.rs (the hook record): `brocanth_middleware::FlashPlugin`
- config/<env>.toml: `[flash]`
- AGENTS.md: `## Flash`

With `rsx`, the root layout `src/pages/layout.rsx` carries
`<div id="flash" role="status"></div>`. The `rsx` module writes it: the
layout is its file.

## What the plugin does

A page writes a message and redirects:

```rust
if let Some(flash) = ctx.hooked_mut::<brocanth_middleware::Flash>() {
    flash.success("Saved.");
}
Ok(Response::empty(303).with_header("location", "/"))
```

- The response hook puts the messages in a cookie, signed with
  HMAC-SHA256. The next request hook reads them back.
- The next HTML response with `<div id="flash"` gets the messages inside
  that element, escaped. Then the hook clears the cookie.
- A redirect, a JSON answer or a partial that has no flash element keeps
  the messages for the next page that has one.
- `flash.now(level, text)` shows a message on this response only: a form
  drawn again with no redirect.
- `flash.take()` gives the messages to a page that draws them itself.

## Hooks

| Hook | Toggle | What it does |
|---|---|---|
| `read` | required | reads the messages the last response left in the cookie |
| `write` | required | draws the messages into `<div id="flash">` and sets or clears the cookie |

## Settings

Section `[flash]`. Environment variable: `$<APP>_FLASH__<KEY>`. Flag: `--flash.<key>`. Rename or hide a flag in `src/modules/flash.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `cookie` | text | `"flash"` | `--flash.cookie` | The cookie this hook owns. |
| `key_env` | text | `"FLASH_KEY"` | `--flash.key_env` | The environment variable that holds the signing key. The name, not the key. |
| `secure` | bool | `true` | `--flash.secure` | Send the cookie over HTTPS only. |

## Decisions

- **A cookie of its own, not a session row.** A flash is a few words for
  one browser for one request. A cookie carries that with no table and no
  sweep, and it works for a visitor with no session. Rails keeps its flash
  in the cookie session for the same reason.
- **Signed.** The text shows in the site's own page. An unsigned cookie
  set from a sibling subdomain could make the site say "call this number".
- **Without a key, a random key per process.** The cost is a lost message
  when a fleet sends the next request to another instance, not a refusal
  to start. Set `$FLASH_KEY` with more than one instance.
- **A message waits for a page that can show it.** Django's messages work
  the same way.

## Read more

- Source: [`crates/brocanth-middleware/src/flash.rs`](../../crates/brocanth-middleware/src/flash.rs),
  [`crates/brocanth-middleware/src/scaffold.rs`](../../crates/brocanth-middleware/src/scaffold.rs)
