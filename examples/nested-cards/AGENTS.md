# Working in Nested Cards

A brief for a coding agent. Written by `brocanth create`, so the lists
below are this project's and not the framework's in general.

## Check your work

Run these after every change, in this order. They are ordered by how
fast they fail.

```
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The route table is `pages::router()` and the test beside it asserts the
shape of the directory. Read it rather than guessing what covers a
request.

## What this binary contains

- `core`  the router, the context, the plugin seams
- `rsx`  `.rsx` templates compiled into the binary; without it a page builds a String
- `rsx-htmx`  a click swaps what changed; the server renders from the shared layout down

**Absent:** `logging`, `config`, `sessions`, `model`, `cache`, `commands`, `rsx-tailwind` — not compiled, not linked, absent from `Cargo.lock`. Do not write code that uses them; add the dependency and name the plugin in `main` first.

That is the framework's central rule: a capability nobody asked for is
not compiled and not linked. If you need one, add it to `Cargo.toml`
*and* name its plugin in `src/main.rs`. Nothing registers itself.

## Adding a route

A route is a file. `src/pages/about.rs` is `/about`;
`src/pages/users/[id].rs` is `/users/{id}`. **Write the file. That is
the whole of it** -- no `mod` line, no row to register, because there is
no list. `brocanth_pages::pages!` in `src/pages/mod.rs` reads this
directory while the crate compiles and writes both.

The verb comes off the handler's name: `handle` is GET, then
`handle_head`, `handle_post`, `handle_put`, `handle_patch`,
`handle_delete`, `handle_options`. A `pub fn handle_*` outside that
list is a build error naming the list, rather than a handler nothing
reaches; a page with no handler is a build error too. A private
`fn handle_*` is a helper and is left alone.

**Adding a file does not rebuild on its own** -- nothing referenced it
yet. Run `touch src/pages/mod.rs` after creating a page or a layout.
Editing or deleting one is noticed.

A handler returns a value and never writes a response itself. That is
what makes a half-written body on an error impossible rather than
unlikely.

## Views

The markup for a route is a `.rsx` file **next to the page that renders it**:
`src/pages/index.rsx` beside `src/pages/index.rs`. There is no second tree and
nothing to register -- `rsx::load_dir!("src/pages")` in `src/pages/mod.rs`
compiles them all into the binary. One file is one module, under `views`, so
`index.rsx` is `views::index` and `users/[id].rsx` is `views::users::_id_`.

### Layouts

`layout.rsx` wraps every page in its directory **and every directory below**,
and a page never names it: which layout wraps a page is a property of where
the page is. That is the Crystal page_router's rule for `layout.cr`.

They stack root to leaf, and **the chain is not written down anywhere**.
`html!` is a macro: it knows which file it was written in, walks up to
`src/`, and wraps the page in every `layout.rsx` it passes. So
`src/pages/users/layout.rsx` wraps the pages under `users/` inside the root
layout, and deleting that file removes exactly that layer with no other edit.

Every page uses the same import and the same call, whatever directory it is
in. Do not add a chain of your own; if a page is wrapped wrongly, the
directory tree is what to change.

A layout takes `children: Element`, and may also take `layout: &str`, which
`html!` fills with the URL prefix that layout covers. Those two are the only
parameters it fills, because it cannot know what a third one would mean.
A value every page shares comes from a `const` -- the root layout reads
`pages::TITLE` that way. A value belonging to one request belongs on the page
component, which the handler builds and which already has the context.

For a fragment that is not a document -- an htmx swap, a partial -- call
`render` instead of `html!`: the same thing with no shell.

**Adding a `layout.rsx` where there was none does not rebuild anything**,
because nothing referenced the file yet. Run `touch src/pages/mod.rs` after
creating one. Editing or deleting an existing layout is noticed.

A component is a function returning `Element`. Its parameters are its contract
with the handler, and a caller fills them as struct fields, so a field the
component does not declare is a compile error rather than a blank in the page.
A parameter of type `Element` receives children, which is all a layout is.

Reach a template from a handler with `html!`, and with nothing else:

```rust
pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    html!(ctx, views::index::Index { title: TITLE })
}
```

It returns the handler's own `Result`, so there is no `Ok(..)` to write and a
template that bails propagates with `?`. For another status or a header,
compose with the other return shapes: `Ok((html!(ctx, tpl)?, 404))`.

Adding a page with markup is two files in one directory -- `<name>.rs` and
`<name>.rsx` -- and nothing else anywhere. Neither needs a line in
`src/pages/mod.rs`, because the directory is the registry.

Not every response is a page. A redirect is `Response::empty(303)` with a
header and no template anywhere near it.

An interpolated value is escaped on the way out. Templates are read at
**compile** time, so an edit needs a rebuild, and a template that does not
parse is a build failure on its own line rather than a 500.

## Static files

Two directories, and the difference matters. `assets/` is **fingerprinted**:
`site.css` is published at `/css/site.<hash>.css` and cached for a year.
`public/` is served **verbatim**, for names that cannot move -- `robots.txt`,
`favicon.ico`, anything linked to from outside.

Never write either URL by hand. `crate::assets::url("css/site.css")` takes the
logical path and returns the published one, and a name nothing publishes
panics there rather than 404ing in a browser.

A basename starting with `_` is an input and is never published, which is what
`assets/css/_tailwind.css` is.

Both directories are embedded in the binary. **Adding a file where there was
none does not rebuild anything**; run `touch src/assets.rs` after creating one.

## Partial navigation

Link to a route with the `Link` component, never a bare `<a>`:

```
<Link to={&crate::pages::routes::users::id::path(7)}>User 7</Link>
```

`to` takes what a route helper returns, so a link to a route that does not
exist is a compile error. It renders a real `<a href>` with `dp-boost` on it,
so the page still works with no JavaScript.

A layout marks what a swap may replace with `dp-layout={layout}`, on the
element wrapping **exactly** the children. Anything inside that element is
replaced with them, so a sidebar or a scroll position belongs outside it.

A request carrying `DP-Partial: n` drops the outermost `n` layouts, and
`html!` resolves that into a `match` over the chain. A handler never reads the
header and never changes for it.

## House style

- A comment says **why**, next to the decision, and what was chosen
  against. Not what the line does.
- A test name is a sentence describing the guarantee it protects.
- Prefer a type that cannot be built wrong over a check that can be
  forgotten.
- State a limit where someone would look for it rather than leaving it
  to be discovered.

## Framework reference

`docs/llm-authoring.md` in the brocanth checkout is the full brief: what
cannot compile and can therefore be relied on, what can still go wrong
and therefore needs a test, and the crate map.
