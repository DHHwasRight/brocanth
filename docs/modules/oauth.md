# `oauth`

An OAuth 2.0 client for third-party APIs (not sign-in); pick its parts below.

The wizard calls it: *Call GitHub, Google, Microsoft or GitLab APIs for your users*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-oauth-client` |
| Without it | no OAuth client; a call to a third-party API carries a token you manage by hand |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/oauth.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | `AgentsSection` `oauth`: how a third-party API is called |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

The parent brings the crate and one rule: a token is a password, so it goes
through `OauthClient::http` and never into a log, a page or a URL. The
children are the parts:

- [`oauth-client`](oauth-client.md): the connect pages, the token store and
  the client. Ticked with the parent.
- [`oauth-client-memory`](oauth-client-memory.md): the tokens in memory, for
  tests. Off until you tick it.

**Why a parent:** a key with a hyphen names its parent (the CLI's naming
test), so `oauth-client` needs one. The parent is the crate, the way
`webhooks` is the crate of `webhooks-in` and `webhooks-out`.

**Why not `auth-oauth`:** `auth-oauth` signs a person in with a provider
and keeps no token. This module keeps a token to call the provider's API
later, for a person who is already signed in. The two can run side by
side; they share no code and no cookie.

## What it writes

Files: none.

Slots:

- AGENTS.md: `## Third-party APIs`

## Read more

- [brocanth-oauth-client](../../crates/brocanth-oauth-client/README.md)
- Source: [`crates/brocanth-oauth-client/src/scaffold.rs`](../../crates/brocanth-oauth-client/src/scaffold.rs)
