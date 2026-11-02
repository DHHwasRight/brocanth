# `client`

A subcommand that calls this application's own API, built from its routes.

The wizard calls it: *A command line that talks to this application*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | [`input`](input.md): the client is built from what each page takes and gives, and that table is what `input` writes; [`commands`](commands.md): the client is a subcommand, and a project with no command line has nowhere to put one |
| After | [`input`](input.md), [`commands`](commands.md) |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-client` |
| Without it | no client; calling this application means writing the URL and the JSON by hand |

## What it writes

Files:

- `src/commands/api.rs`

Slots:

- src/commands/mod.rs: `pub mod api;`
- AGENTS.md: `## The client`

## Settings

None.

## Read more

- [The surface](../surface.md)
- Source: [`crates/brocanth-client/src/scaffold.rs`](../../crates/brocanth-client/src/scaffold.rs)
