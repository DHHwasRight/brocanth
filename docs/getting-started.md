# Getting started

Every command here was run against this checkout on 2026-10-14, and the output
is pasted rather than recalled. Where something is not built yet it says so
instead of describing what it will do.

## Install the CLI

brocanth is not published, so the CLI comes from this checkout:

```
cargo install --path crates/brocanth-cli
```

That installs one binary, `brocanth`. To work on the CLI itself, skip the
install and use `cargo run -p brocanth-cli --` in front of every command below.

## Make a project

```
brocanth create blog --brocanth-path /path/to/brocanth/crates/brocanth
```

`--brocanth-path` is needed for the same reason as the install: nothing is on
crates.io yet, so the generated manifest has to point at your checkout. **It
takes the core crate's own directory — `crates/brocanth` — not the checkout
root**, which is a virtual manifest cargo will not take a path dependency on.
Passing the root is caught and answered with the command you meant:

```
error[brocanth-path]: /path/to/brocanth is a workspace, not a crate
  = note: cargo cannot take a path dependency on a virtual manifest
  = help: --brocanth-path /path/to/brocanth/crates/brocanth
```

Run with no name and a terminal on both stdin and stdout, `brocanth create`
opens a chooser instead. With a name it runs straight through, because every
other answer has a default.

What it writes:

```
  blog/Cargo.toml                  1455 bytes
  blog/.gitignore                     8 bytes
  blog/rust-toolchain.toml           29 bytes
  blog/mise.toml                    492 bytes
  blog/src/main.rs                  756 bytes
  blog/src/app.rs                  6595 bytes
  blog/src/pages/mod.rs            8838 bytes
  blog/src/pages/index.rs           802 bytes
  blog/src/pages/about.rs          1040 bytes
  blog/src/pages/users/[id].rs     1656 bytes
  blog/src/plugins/mod.rs          3197 bytes
  blog/config/development.toml      560 bytes
  blog/src/commands/mod.rs          582 bytes
  blog/src/commands/serve.rs        592 bytes
  blog/src/plugins/logging/mod.rs  2388 bytes
  blog/AGENTS.md                   3351 bytes
  blog/brocanth.toml                609 bytes

    core          a router handing a request and a context to a handler
    logging       a line per request; the core assumes no log layer
    config        config/<env>.toml, merged; without it nothing links a TOML parser
    commands      this project's own command line, `serve` plus whatever you add
    sessions      not asked for: absent from the source, the binary and the lock file
    model         not asked for: absent from the source, the binary and the lock file
    cache         not asked for: absent from the source, the binary and the lock file
    jobs          not asked for: absent from the source, the binary and the lock file
    realtime      not asked for: absent from the source, the binary and the lock file
    rsx           not asked for: absent from the source, the binary and the lock file
```

Those last lines are the point of the whole design, not a progress
message. A capability nobody asked for is not compiled, not linked, and has no
entry in `Cargo.lock`. `cargo tree` on a fresh project will not show an ORM or
a SQL driver.

### `AGENTS.md`, and why the scaffold writes one

brocanth is meant to be written by a language model as much as by a person, so
every project gets a brief for one. It is generated rather than copied,
because the useful part is **this project's** facts:

```
**Absent:** `sessions`, `model` — not compiled, not linked, absent from
`Cargo.lock`. Do not write code that uses them; add the dependency and name
the plugin in `main` first.
```

A model that cannot see what is missing assumes the framework has it, writes
against it, and then tries to fix the compile error by guessing harder.
Naming the gap turns that into "add the dependency first". The brief also
carries the check-your-work loop, how to add a route, and the house style.

`docs/llm-authoring.md` in this checkout is the longer version: what cannot
compile and can therefore be relied on, what can still go wrong and therefore
needs a test, and a map of the crates.

### Choosing what goes in

```
brocanth create blog --with logging,config,commands,sessions,model,rsx
brocanth create blog --core-only
```

The default is `logging,config,commands`. `--core-only` gives a router, a context
and nothing else.

| `--with` | What it adds |
|---|---|
| `logging` | A request logger. The core assumes no log layer at all. |
| `config` | `config/<env>.toml`, four sources merged, typed per section. |
| `sessions` | Cookie sessions, and a `Session` on the context. |
| `model` | The model layer: SeaORM, validations and migrations. **Postgres by default** — see the warning at the end. |
| `commands` | A `manage` binary whose subcommands are your own structs. |
| `rsx` | HTML from `.rsx` templates beside the pages, with layouts. See below. |
| `rsx-tailwind` | A CSS build over the class names in your templates. Needs node. |
| `rsx-htmx` | A click swaps what changed, instead of reloading the document. |

The last two are part of `rsx` and are named for it, because a command line is
flat: `--with rsx-htmx` has to say on its own what an indent says on a screen.
Naming one without `rsx` is an error that prints the line that works.

### Adding a module later

```
cd blog
brocanth add jobs
cargo build
```

`add` is the same generator, with one more entry on the list in
`brocanth.toml`. That file is the project's record of what it is made of:

```toml
[project]
name = "blog"
title = "Blog"

[modules]
enabled = ["logging", "config", "jobs", "commands"]

[modules.jobs]
files = ["src/plugins/jobs/"]
config = ["jobs"]
crates = ["brocanth-jobs"]
```

Four kinds of file come out of a run, and which is which is fixed:

| File | What `add` does |
|---|---|
| `src/plugins/mod.rs`, `src/commands/mod.rs`, `src/main.rs`, `brocanth.toml`, `AGENTS.md` | Rewrites it. Each carries a digest, and an edited one stops the command. |
| `Cargo.toml`, `config/<env>.toml` | Appends. Every other byte stays where it was, comments and all. |
| `src/plugins/<name>/`, `src/pages/`, `src/app.rs` | Yours. Written when absent, never rewritten. |

An owned file you have edited stops the run, with a diff of what would have
gone. `--force` discards the edit, after you have seen it.

A module whose arrival would change a file of yours is refused, and the
command says which file and shows the change. `config` is the clear case: it
decides whether every *other* plugin's settings struct derives `Deserialize`,
and those structs are yours. Adding it to a project that already has plugins
means making that change by hand, and the command prints it.

`src/app.rs` is the exception. What lands there is a middleware mount, and a
mount is a decision -- the pattern is the application's to choose, which is
why that file is yours. So the line is printed for you to place, and the
project compiles and runs without it:

```
! src/app.rs is yours, and one line belongs in it
  a mount is a decision, so the generator does not make it

                  setup.router().mount("/{*rest}", "request_log");
    +             setup.router().mount("/users/{*rest}", "session");
                  Ok(())
```

There is no `brocanth remove` yet. The per-module record above is written so
that there can be one: it says what each module put in the project, so a
later `remove` deletes what was written rather than what a rule guesses.

## Run it

```
cd blog
cargo run
```

```
[logging] level debug
blog: plugins: [config, logging]
GET / -> 200
GET    /           200 <!doctype html><title>Blog</title><h1>Blog</h1><
GET /users/7 -> 200
GET    /users/7    200 <!doctype html><title>User 7</title><h1>User 7</
DELETE /about      405 Allow: GET, HEAD
GET    /nowhere    404 Not Found
```

**That is a real socket.** Open `http://127.0.0.1:3000` in a browser, or:

```
$ curl -i -X DELETE http://127.0.0.1:3000/about
HTTP/1.1 405 Method Not Allowed
allow: GET, HEAD
```

A path that exists with a verb it does not answer is a 405 carrying an
`Allow` list, never a 404.

## See what this project can do

```
brocanth --help
```

```
Commands:
  create  Scaffold an application
  serve   Serve the application over HTTP
  help    Print this message or the help of the given subcommand(s)
```

`create` is `brocanth`'s own and the only one it has. Everything below it is
**your project's**, read out of the built binary — so a command you add appears
here once it has been compiled. Nothing is listed that this project cannot run.

Outside a project the same help shows `create` alone, because there is no
project to ask.

`-D` has exactly one job, and it belongs to `create`. Run with no name, the
chooser opens; `-D` then prints the command line that would have given the
same answers with nothing to ask:

```
$ brocanth -D create
debug: brocanth create blog --title Blog --with logging,config,commands --git
```

Copy that into a script and the same project comes out. It is written in
full rather than minimally, because a line that leans on a default changes
meaning when the default does. A run that was already driven by flags prints
nothing: it was already the line.

## Add a page

Write a file. That is the whole of it.

`src/pages/contact.rs`:

```rust
//! `/contact` -- one file, one route.

use crate::pages::{Ctx, Error, IntoResponse};

pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    Ok("<!doctype html><title>Contact</title><h1>Contact</h1>")
}
```

Then `brocanth serve` and `curl http://127.0.0.1:3000/contact`. There is no
`mod` line to add and no row to register, because **there is no list**.
`src/pages/mod.rs` has one line where a table used to be:

```rust
brocanth_pages::pages!("src/pages");
```

That macro reads the directory while the crate compiles and writes both: the
`mod` lines, and a `router()` with one row per verb per page. So adding a page
is adding a file, renaming one renames its route, and deleting one deletes it.

**The verb comes off the handler's name.** `handle` is GET, `handle_post` is
POST, and the rest of the table is `handle_head`, `handle_put`,
`handle_patch`, `handle_delete`, `handle_options`. Two handlers in one file
are two rows:

```rust
pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> { .. }       // GET
pub fn handle_delete(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> { .. } // DELETE
```

```
$ curl -i -X PUT http://127.0.0.1:3000/blog/hello
HTTP/1.1 405 Method Not Allowed
allow: GET, HEAD, DELETE
```

The page's source is **parsed** to find those, not scanned for text — the
scaffold's own files mention `pub fn handle` in a doc comment, and a route
table that depended on where a comment sits would be a table nobody could
trust.

Two mistakes it refuses rather than passes over:

```
error: `blog/[slug].rs` has `pub fn handle_json`, which is not a verb: the
       framework reads the verb off the name, and the names it reads are
       handle, handle_head, handle_post, handle_put, handle_patch,
       handle_delete, handle_options. Rename it, or make it private if it
       is not a handler
```

```
error: `empty.rs` declares no handler, so nothing would route to it; write
       `pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error>`,
       or delete the file
```

A private `fn handle_anything` is a helper and is left alone. `mod.rs` is the
table rather than a page, and is skipped.

> **Adding a file does not rebuild on its own.** Nothing referenced it yet, so
> cargo cannot see it. `touch src/pages/mod.rs` after creating a page or a
> layout. Editing or deleting an existing one is noticed, because every file
> the macros use is `include_bytes!`d by the expansion.

A handler returns a value and never writes a response itself, which is what
makes a half-written body on an error impossible rather than unlikely.

## What a handler returns

`Result<impl IntoResponse, Error>`. Two halves, and both earn their place.

**The `Result`** is why `?` works. Anything that fails converts into `Error`
through a blanket `From`, so a parse or a query is one character:

```rust
let id: u64 = ctx.require("id")?.parse()?;
```

One place turns a failed handler into a response — what Echo calls an
`HTTPErrorHandler` — and it lives in `dispatch` in `src/main.rs`. The
difference from Echo is that the handler has no writer, so there is no
already-sent 200 for the error path to fail to take back.

**The `impl IntoResponse`** is the Crystal original's idea: a page returns the
simplest thing that says what it means. Each handler still returns exactly one
type; the trait is what lets *different* handlers return *different* ones.

| Return | Means |
|---|---|
| `Ok("<h1>Hi</h1>")` or `Ok(string)` | 200 with that body |
| `Ok(())` | 200, no body |
| `Ok(404u16)` | that status, no body |
| `Ok((body, 201))` | body and status |
| `Ok((body, 201, headers))` | body, status and headers |

Past that, build the struct. Rust requires **every** field — there are no
implicit zero values, so omitting one is a compile error, not a `0` — and `..`
fills in the rest:

```rust
Ok(Response { status: 201, ..Default::default() })          // 201, no body
Ok(Response { body: html, ..Default::default() })           // 200 with a body
Ok(Response { status: 404, ..Response::ok(page) })          // from any value
```

`Default` is written by hand rather than derived, and that matters: a derived
one would give `status: 0`, which is not an HTTP status, so
`Response { body: .., ..Default::default() }` would compile and be wrong.

For several headers, chain instead — Rust has no variadic generics, so a tuple
return is fixed-arity and this is the unbounded form:

```rust
Ok(Response::empty(303).with_header("location", "/users/7"))
```

## HTML from templates: `--with rsx`

Without it, a page builds the String it returns and the manifest says so:

```
#   rsx              no template engine; a page builds the String it returns
```

With it, the markup for a route moves into a `.rsx` file **next to the page
that renders it**, and is compiled into the binary:

```
src/pages/layout.rsx          wraps this directory and everything below
src/pages/index.rs            the handler
src/pages/index.rsx           the markup
src/pages/about.rs
src/pages/about.rsx
src/pages/users/layout.rsx    wraps this directory, inside the root layout
src/pages/users/[id].rs
src/pages/users/[id].rsx
```

That is the Crystal `page_router` layout, and both halves of it carry over: a
route is a file, so the markup is the file beside it; and `layout.rsx` covers
its directory and every directory below, so **a page never names its shell**.

### Layouts

**`src/pages/layout.rsx`** — a layout is not a special kind of thing. It is a
component with a parameter of type `Element`, which is what the page below it
fills:

```
pub fn Layout(title: &str, children: Element) -> Element {
    <!DOCTYPE html>
    <html lang="en">
        <head>
            <meta charset="utf-8">
            <title>{title}</title>
        </head>
        <body>
            <header><a href="/">{title}</a></header>
            <main>{children}</main>
        </body>
    </html>
}
```

**`src/pages/index.rsx`** — the page, and only the page. It does not know the
document above exists:

```
pub fn Index(title: &str) -> Element {
    <h1>{title}</h1>
    <p>…</p>
}
```

**They stack, root to leaf, and nothing declares the chain.**
`src/pages/users/layout.rsx` wraps the pages under `users/` *inside* the root
layout, not instead of it. `/users/7` comes back as root → users → page:

```html
<main>
  <section class="users">
    <nav><a href="/">home</a></nav>
    <h1>User 7</h1>
```

`/about` is not under `users/`, so it gets the root layout alone.

**There is no chain written down anywhere.** `html!` is a macro that reads
the directory tree while the crate compiles: it knows which file it was
written in, walks up to `src/`, and wraps the page in every `layout.rsx` it
passes. So the proof is deleting one. Remove `src/pages/users/layout.rsx`,
change nothing else, rebuild:

```html
<main>
  <h1>User 7</h1>
```

One layer gone, no source edited. Three levels work the same way — a page in
`src/pages/blog/archive/` with a `layout.rsx` in `blog/` and in `archive/`
renders root → blog → archive → page, and the handler is still one line.

> **The one thing cargo cannot see.** Adding a `layout.rsx` where there was
> none does not rebuild anything, because nothing referenced the file yet.
> `touch src/pages/mod.rs` and it is picked up. Editing or deleting an
> existing one is noticed: each layout the macro uses is `include_bytes!`d by
> the expansion.

**`src/pages/index.rs`** — the handler. One line, and no `Ok(..)`:

```rust
pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    html!(ctx, views::index::Index { title: "Blog" })
}
```

`html!` puts the page inside the layouts that cover it, and expands to
`render`. It takes the context because the request can ask for part of a
document rather than all of it -- see `rsx-htmx` below; without that choice the
context is read and the answer is always the whole thing. For a fragment that is not a document at all — an htmx swap, a
partial — call `render` directly and get no shell:

```rust
render(views::users::row::Row { user })
```

`render` is a function rather than a trait impl for three reasons worth
knowing:

- **It returns the handler's own `Result`.** A template that bails propagates
  with `?` instead of being swallowed into a blank 200. An `into_response` that
  cannot fail would have had to swallow it.
- **A blanket `impl<T: rsx::Template> IntoResponse for T` does not compile.**
  `()`, `Option<T>` and `Vec<T>` are templates too, so it overlaps the impls in
  the table above.
- **It is where the layout goes.** A blanket impl would not know which page it
  was rendering, so it could not choose one.

A layout component takes `children: Element`, and may also take
`layout: &str`, which `html!` fills with the URL prefix that layout covers --
`/` for the root one, `/users` for `src/pages/users/layout.rsx`. Those two are
the only parameters `html!` fills, because it cannot know what a third one
would mean. A layout that does not want the prefix does not declare it, and
nothing is passed. A value every page shares
comes from a `const` — the generated root layout reads `pages::TITLE` that way
— and a value belonging to one request belongs on the page component, which
the handler builds and which already has the context.

For another status or a header, compose with those same shapes:

```rust
Ok((html!(ctx, views::errors::NotFound {})?, 404))
Ok(html!(ctx, tpl)?.with_header("cache-control", "no-store"))
```

And not every response is a page. The generated POST is a redirect, with no
template anywhere near it:

```rust
Ok(Response::empty(303).with_header("location", format!("/users/{id}")))
```

### What the compiler catches here

`src/pages/mod.rs` gains one block:

```rust
pub mod views {
    rsx::load_dir!("src/pages");
}
```

A module of its own, because the generated `pub mod index` would otherwise
collide with the `pub mod index;` naming the page. A file whose name is not an
identifier gets the nearest one, so `users/[id].rsx` is `views::users::_id_`.

That macro reads the tree **while the crate is compiling**. So:

| Mistake | When you find out |
|---|---|
| Template does not parse | build failure, on the `.rsx` line |
| A field the component does not declare | build failure, naming the ones it takes |
| Children given to a component with no `Element` parameter | build failure, with the signature to write |
| A renamed component | build failure in the handler |

Misspell a parameter and the build stops on the template, not on generated
code:

```
error: component `Layout` has no parameter `titel`
  --> blog/src/pages/index.rsx:4:13
   |
 4 |     <Layout titel={title}>
   |             ^^^^^^^^^^^^^
   = help: `Layout` accepts: title, children
```

That is a cross-file lookup: `Layout` is in `layout.rsx` and the call is in
`index.rsx`. `load_dir!` compiles the tree as one unit, so it knows both.

Adding a page with markup is therefore two files in one directory and nothing
else at all — neither the page nor the template is registered anywhere,
because the directory is the registry.

Nothing is read from disk when a request arrives, and there is no template
directory to ship. The cost is that an edit needs a rebuild — each file is
`include_bytes!`d by the expansion, which is what makes cargo notice.

An interpolated value is escaped. Writing markup verbatim needs a different
hole, so an injection is something you can grep for.

**rsx comes from its own repository**, not from your brocanth checkout, so
`--brocanth-path` does not apply to it and the generated manifest reads:

```toml
rsx = { git = "https://github.com/DHHwasRight/rsx.git", branch = "main" }
```

## Inside rsx: `rsx-tailwind` and `rsx-htmx`

```
brocanth create blog --with rsx,rsx-tailwind,rsx-htmx
```

The chooser and the flags disagree here, deliberately:

| | The chooser | `--with` |
|---|---|---|
| `rsx` ticked | both appear under it, already ticked | `--with rsx` is rsx and nothing else |
| `rsx` unticked | both leave the screen; the tick is remembered, not counted | `--with rsx-tailwind` is an error |

Somebody in a terminal building an HTML application nearly always wants
styling and nearly always wants a click not to reload the page, and can see
both rows and untick them. A script gets what it named, because a flag that
quietly brings two more is a flag you cannot read.

### Static files

Both choices need files the browser fetches, so both bring `src/assets.rs`:

```rust
brocanth_assets::assets!("assets", "public");
```

Two directories, two rules:

| File | Served at | `cache-control` |
|---|---|---|
| `assets/css/site.css` | `/css/site.cb1cb6eb.css` | `max-age=31536000, immutable` |
| `public/dpswap.min.js` | `/dpswap.min.js` | `max-age=0, must-revalidate`, then **304** |

`assets/` is fingerprinted with eight hex digits of the file's SHA-256, so it
can be cached for a year and a new build invalidates it by having a different
name. `public/` keeps the name it was given, because `robots.txt`,
`favicon.ico` and anything linked to from outside cannot move.

Write neither URL by hand:

```
<link rel="stylesheet" href={crate::assets::url("css/site.css")}>
```

`url` takes the logical path — what the file is called on disk — and returns
the published one. A name nothing publishes panics there, with the fix in the
message, rather than 404ing in a browser.

A basename starting with `_` is an input and is never published:
`assets/css/_tailwind.css` is read by the CSS build and has no URL.

Both directories are embedded, like the templates and the route table, so
`serve` works from any directory and nothing goes missing after the build
passed. Static files are answered before the router runs, so a font never
reaches a handler and never costs a route.

> **The same thing cargo cannot see.** A file added where there was none does
> not rebuild anything, because nothing referenced it yet. `touch
> src/assets.rs` and it is picked up.

### `rsx-tailwind`

```
assets/css/_tailwind.css   the input; never published
assets/css/site.css        the output, and what the layout links to
src/commands/css.rs        the build, as a subcommand of this project
package.json               one dev dependency, @tailwindcss/cli
mise.toml                  mise css, and mise css:watch
```

The build is a command and not a shell script:

```
brocanth css          once, minified
brocanth css -W       again on each change
```

`rsx-tailwind` therefore needs `commands`, and `brocanth create` says so
rather than writing a project with nowhere to put the command.

The globs are written out rather than left to Tailwind to find:

```css
@source "../../src/pages/**/*.rsx";
@source "../../src/**/*.rs";
```

`.rsx` is not an extension Tailwind knows, and a class name built in a Rust
string is not one it would look for either. An explicit glob states the
contract.

The command rewrites `src/assets.rs` when it is done, because a new
stylesheet is a new binary: the CSS is embedded, so nothing changes until the
crate rebuilds. It runs the CLI from `node_modules` and does **not** fall back
to `npx`. A fallback that downloads some other version hides the one thing
worth knowing, which is that nobody ran `npm install`.

### `rsx-htmx`

A click swaps what changed. Three parts, and the first is an anchor.

**`<Link>`**, in `src/pages/link.rsx`:

```
<li><Link to={&crate::pages::routes::users::id::path(7)}>User 7</Link></li>
```

```html
<a href="/users/7" dp-boost>User 7</a>
```

A real link, enhanced second. With JavaScript off, to a crawler, and on the
first load of any page the browser does what it always did. `to` takes what
the route helpers return rather than a string, so a link to a route that does
not exist is a compile error instead of a 404 somebody finds later.

**`dp-layout`** marks the element a swap may replace. It goes on the element
wrapping exactly the children and nothing else:

```
pub fn Layout(layout: &str, children: Element) -> Element {
    <section class="users">
        <nav><a href="/">home</a></nav>
        <div dp-layout={layout}>{children}</div>
    </section>
}
```

The `<nav>` is outside it, so it survives a navigation within `users/`.
Anything inside it is replaced along with the page — a sidebar in there would
lose its scroll position on every click.

**`DP-Partial`** is the answer. The script compares the page it is on with the
page it is going to, finds the deepest layout they share, and asks for that
much:

| Request to `/users/7` | What comes back |
|---|---|
| no header | the whole document |
| `DP-Partial: 1` | the `users/` layout, and the page inside it |
| `DP-Partial: 2` | the page alone |

Each step drops one more layer from the outside. `html!` already knows the
chain at compile time, so it expands into a `match` over it and the server
renders from the shared layout down. depoth builds a tree per depth ahead of
time because it has no server to ask; this has one, so the depth is a number
in a header and the answer is computed per request.

You can see all three by hand:

```
curl -s localhost:3000/users/7
curl -s -H "DP-Partial: 1" localhost:3000/users/7
curl -s -H "DP-Partial: 2" localhost:3000/users/7
```

**dp-swap is vendored, not fetched.** `public/dpswap.min.js` is 13 kB and
comes from the CLI's own `vendor/` directory, so `create` finishes on a
machine with no network.

## Add a command

Tasks are the part that is finished. A struct with `#[cmd]` on it becomes a
subcommand of your project's own binary — there is no list to add it to, because
registration happens at link time.

`src/commands/mod.rs` already has one:

```rust
/// Say hello, to prove the wiring works.
#[cmd]
#[derive(clap::Args)]
pub struct Greet {
    /// Who to greet.
    #[arg(long, default_value = "world")]
    pub name: String,
}

impl Command for Greet {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        println!("hello, {} (env: {})", self.name, ctx.env());
        Ok(())
    }
}
```

Two ways to run it. Through the globally installed CLI, from anywhere inside
the project:

```
$ brocanth greet --name max
hello, max (env: development)
```

Or by running the binary, which is the same thing without the walk-up:

```
$ cargo run -- greet --name max
```

There is no cargo alias. A project's command line is a framework concern and
cargo is the build tool; hooking one into the other conflates them.

`brocanth` knows one command of its own, `create`, and treats everything else
as the project's: it walks up
for `brocanth.toml`, runs `cargo build`, asks `cargo metadata` where the binary
landed, and `exec`s it. So the command owns the terminal, its exit code is the one
your shell sees, and Ctrl-C reaches the command rather than a wrapper.

The help text comes from the doc comment, because `clap::Args::augment_args`
copies it onto the command. There is no second description to keep in step.

Put a new command in its own file under `src/commands/`, add a `mod` line, and it is
a subcommand.

### The one failure here that is silent

**A crate that is never referenced is dropped by the linker, and its commands go
with it.** Your own crate is fine — `main` is in it. A *third-party* crate whose
only job is to contribute commands is not: nothing in your code mentions it, so
nothing keeps it. Reference it explicitly in `src/bin/manage.rs`:

```rust
use some_task_pack as _;
```

Without that line it compiles, the tests pass, and the commands are simply absent
from `--help`.

## What is not built yet

**The global tool scaffolds and nothing else.** `brocanth` has one command of
its own, `create`. There is no `routes`, no `check`, no `build`, no `dev` and
no `generate`: they were removed, because a project's command line belongs to
the project, where it can be tuned without forking anything.

So adding a page is writing a file, as above, and watching for changes is
whatever you already use — `cargo watch`, `mise`, an editor task. If you want
`myapp routes`, write it: a struct with `#[cmd]` under `src/commands/` is a
subcommand, and the route table is `pages::router()`.

## One thing to know if you pass `--with model`

The model layer now defaults to **Postgres**. The generated manifest names
`brocanth-model` with no features, so a new project gets the Postgres driver
and expects a server. For the small single-machine setup, edit the dependency
yourself:

```toml
brocanth-model = { path = "...", default-features = false, features = ["sqlite"] }
```

The generator does not know to write that, which is a rough edge rather than a
decision.

## Where to go next

- `docs/llm-authoring.md` — the brief for writing brocanth with a language
  model: the 36 mistakes that cannot compile, the ones that still can, and the
  crate map. Worth reading even if you are writing by hand, because it is the
  shortest statement of what the framework guarantees.
- `PLAN.md` — the argument the whole project is built on. §8 (modularity) is
  the one that explains why a scaffold tells you what it left out.
- `crates/brocanth-command/README.md` — commands in full.
- `docs/async-handlers.md` — how a route gets to `.await`, what it costs, and
  what is still missing.
- `docs/pocketbase-surface.md` — the thing brocanth is measured against, with
  what is deliberately declined.
