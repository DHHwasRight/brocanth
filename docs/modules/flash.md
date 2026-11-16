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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-middleware` (feature `config` with `config`) |
| Without it | no flash; a page a form redirects to cannot say what the form did |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/flash.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | `AgentsSection` `flash`: how a page sets a message for the next page |
| Collects | none |
| Exports | none |
| Hooks | `read` (Identify, required): read the messages the last response left in the cookie; `write` (Identify, required): draw the messages into `<div id="flash">` and set or clear the cookie |
| Options (build time) | none |
| Settings (run time) | `[flash]`: `cookie`, `key_env`, `secure` |
<!-- end generated: capabilities -->

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
