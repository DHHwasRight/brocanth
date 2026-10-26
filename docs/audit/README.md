# The double-entry table

Four frameworks, every capability each one ships, and whether brocanth has
it. **285 rows**, audited against the code at `5017473` by four independent
agents, one per framework.

| Framework | Version | Rows | yes | partial | no | n/a |
|---|---|---:|---:|---:|---:|---:|
| [Rails](rails.md) | 8.1.4 | 95 | 8 | 42 | 44 | 1 |
| [Django](django.md) | 6.1 | 88 | 22 | 43 | 20 | 3 |
| [Next.js](nextjs.md) | 16.4 | 51 | 7 | 17 | 17 | 10 |
| [PocketBase](pocketbase.md) | 0.40.6 | 51 | 3 | 30 | 12 | 6 |
| **Total** | | **285** | **40** | **132** | **93** | **20** |

`n/a` is a deliberate non-goal, not a gap: no client-side component
runtime, no stateful process per connection, policy rules compile to SQL.
Twenty rows, and the Next.js audit was told explicitly not to pad that
column with anything merely because React does it.

## The finding all four made

**`partial` is the largest column, and it has one shape.** A crate is
written, tested, and no generated project can reach it. Eighteen of
PocketBase's thirty partials are that; so is the 5,274-line admin; so is
the finished HTTP test client, which no project gets because `create.rs`
emits no `[dev-dependencies]` at all.

brocanth's debt is **reachability, not absence**. That is why the first
things to build are seams rather than features.

## Ranked by how many audits put it in their top ten

| Votes | Work | Size |
|---:|---|---|
| 4 | **Mount route bundles in the generated `src/app.rs`.** `App::mount_routes` and `route_plan` are written and tested; a generated app dispatches `pages::router()` alone. Unblocks `resource`, the admin, and lets OpenAPI stop pretending to be a page | 1–2 days |
| 3 | **A streaming body reachable from a page**, and the SSE transport on top of it | 2–5 days |
| 3 | **A mailer.** `brocanth-auth` returns what to send and nothing can send it, so the module that just reached the menu cannot finish a password reset | 3–7 days |
| 3 | **Migrations and database tasks.** The `model` plugin publishes `Db { url }` and opens no connection; there is no migrations directory and no `migrate` command | 3–5 days |
| 2 | **CSRF.** The only `csrf` in the tree is OAuth2's `state`, a different mechanism | 1–2 days |
| 2 | **The admin on the menu** (after the mount seam) | 2–4 days |
| 2 | **`brocanth-test` as a generated dev-dependency** | hours |
| 2 | **Multipart and file uploads end to end** | 3–5 days |

## Two corrections this audit makes to our own records

**`G1: the response body cannot stream` is recorded "done" and is
half-true.** The core streams: `Body::{Buffered, Streaming}`,
`on_headers`/`on_body`, `StreamGate`. A *generated project* cannot, because
its `Response.body` is a `String` and `sent()` hands it to
`Response::html`. Two audits disagreed about this and both were right about
a different layer. Verified here.

**There is no `brocanth-pgserver` branch and there never was.**
`git log --all` finds nothing. It had been carried as an open work item.
The `embedded-postgres` branch exists, is merged, and holds the chooser and
the realtime broker — not an embedded database.

**`docs/framework-gaps.md` §2.2 is superseded by this directory.** It was
wrong in both directions: it said there is no HTTP test client (there is)
and that auth is unwired (it is on the menu), while calling layouts plainly
present when they cannot read the request.

## Smaller than a day, and nobody is doing them

- `src/pages/sitemap.xml.rs` already routes to `/sitemap.xml`. A dynamic
  sitemap works today, by accident of the file-extension rule.
- `crates/brocanth-cli/src/modules/model.rs:61` and `:120` tell the reader
  to run `brocanth generate model`, which `main.rs` does not define.
- A `brocanth routes` command is promised in six doc comments and not
  implemented. The table is already enumerable, so it is a printer.
- Three security-header response hooks beside the five in
  `brocanth-middleware`.
