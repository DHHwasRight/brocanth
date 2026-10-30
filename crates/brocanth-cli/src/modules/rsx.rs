//! `rsx`, and the two choices that are part of it.
//!
//! Three modules in one file, because they are one subject: templates, the
//! CSS build that reads the class names out of them, and the navigation that
//! skips the layouts they define. Each is a separate entry on the menu, and
//! each can be left out.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// rsx lives in its own repository, so `--brocanth-path` does not reach it.
/// That asymmetry is on purpose: brocanth is the thing being developed here
/// and wants a path, rsx is an ordinary dependency and is fetched.
pub const REPOSITORY: &str = "https://github.com/DHHwasRight/rsx.git";

/// HTML from `.rsx` templates, compiled into the binary.
pub struct Rsx;

impl Module for Rsx {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "rsx",
            "`.rsx` templates compiled into the binary; without it a page builds a String",
        )
        .outside_runtime()
        .asked_in("pages", "Templates compiled into the binary")
        .krate(Dep::git("rsx", REPOSITORY, "main").note("templates, compiled"))
        .absent(
            "rsx",
            "no template engine; a page builds the String it returns",
        )
    }

    fn contribute(&self, p: &Project, s: &mut Scaffold) {
        // The renderer's half of `src/pages/mod.rs`: the `views` module
        // wrapping `load_dir!`, the `html!` re-export, and the two
        // functions a page's expansion calls. The generator used to write
        // this inline, which is why a project could have exactly one
        // template engine.
        s.at(Slot::PagesRenderers).block(format!(
            "/// The application's name, as the root layout writes it.\n\
             pub const TITLE: &str = {:?};\n\n{PAGES_RSX_RS}\n",
            p.title
        ));

        // Beside the page each one renders, and named after it. The Crystal
        // page_router does the same with `layout.cr`, and the reason carries
        // over: a route is a file, so the markup for a route is the file
        // next to it rather than a mirror tree to keep in step.
        s.file("src/pages/layout.rsx", layout_rsx(p));
        s.file("src/pages/index.rsx", index_rsx(p));
        s.file("src/pages/about.rsx", ABOUT_RSX);
        s.file("src/pages/users/layout.rsx", users_layout_rsx(p));
        s.file("src/pages/users/[id].rsx", USER_RSX);

        // The embedder, when either child asked for static files. Written
        // by the parent and not by the two of them, because a file two
        // modules write is a clash the scaffold reports rather than a file
        // written twice.
        if p.has("rsx-tailwind") || p.has("rsx-htmx") {
            s.file("src/assets.rs", ASSETS_RS);
        }
    }
}

/// Tailwind, built from the class names in the templates.
pub struct Tailwind;

impl Module for Tailwind {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "rsx-tailwind",
            "a CSS build over the class names in your templates; needs node",
        )
        .outside_runtime()
        .asked_in(
            "pages",
            "Tailwind, built from the class names in your templates",
        )
        .part_of("rsx")
        .because("the CSS build reads class names out of `.rsx` files")
        .needs(
            "commands",
            "the CSS build is `brocanth css`, a subcommand, and a project \
             with no command line has nowhere to put it",
        )
    }

    fn contribute(&self, p: &Project, s: &mut Scaffold) {
        s.file("assets/css/_tailwind.css", TAILWIND_CSS);
        // So the first `cargo build` works before anyone has run node.
        s.file("assets/css/site.css", SITE_CSS_PLACEHOLDER);
        s.file("package.json", package_json(p));

        // The CSS build. A command and not `scripts/build-css.sh`, because
        // this project has a command line and a second way to run things is
        // a second thing to keep in step. `needs("commands", ..)` is what
        // makes the line below have somewhere to land.
        s.file("src/commands/css.rs", CSS_RS);
        s.at(Slot::CommandsMod).line("pub mod css;");

        // `assets/css/site.css` is **not** ignored. It is embedded at
        // compile time, so a checkout without it does not build, and "run
        // npm first" is a worse answer than 3 kB in git.
        s.at(Slot::GitIgnore).line("/node_modules");

        // The same build `brocanth css` runs, spelled for a tool that does
        // not know this project has a command line. `cargo run` and not
        // `brocanth`, so a checkout works before anything is installed
        // globally.
        s.at(Slot::MiseTasks).block(MISE_CSS_TASKS);

        let _ = p;
    }
}

/// Partial navigation: dp-swap, a `<Link>`, and layouts the server can skip.
pub struct Htmx;

impl Module for Htmx {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "rsx-htmx",
            "a click swaps what changed; the server renders from the shared layout down",
        )
        .outside_runtime()
        .asked_in("pages", "A click swaps what changed, not the whole page")
        .part_of("rsx")
        .because(
            "`<Link>` is an `.rsx` component, and the partial it asks for is a layout \
             the templates define",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        // `<Link>` is an `.rsx` component, so it is a template like any
        // other and lands beside them.
        s.file("src/pages/link.rsx", LINK_RSX);
        // Vendored into the brocanth binary, written out here. A project
        // that has to fetch a script to finish is a project that fails on
        // a train.
        s.file("public/dpswap.min.js", DPSWAP_JS);
    }
}

// -- what the templates are -------------------------------------------------

/// The templates and the bridge to them: what `rsx` adds to
/// `src/pages/mod.rs`, written into [`Slot::PagesRenderers`].
const PAGES_RSX_RS: &str = r##"/// Every template in this application.
///
/// The `.rsx` files sit next to the pages they render -- `index.rsx` beside
/// `index.rs` -- and `load_dir!` compiles the tree while this crate is being
/// compiled. Nothing is read from disk when a request arrives, and a template
/// that does not parse is a compile error on its own line.
///
/// A module of their own, because `load_dir!` emits a `pub mod index` for
/// `index.rsx` and `pages!` emits one for `index.rs`: unnested they collide.
/// The path is relative to the package root and not to this file, because a
/// proc macro cannot learn where it was written on stable Rust.
///
/// A file whose name is not an identifier gets the nearest one: `[id].rsx` is
/// `views::users::_id_`.
pub mod views {
    rsx::load_dir!("src/pages");
}

/// Renders a page inside the layouts that cover it.
///
/// `layout.rsx` wraps every page in its directory and in every directory
/// below it, and they stack from the root down. A page never names one,
/// because which layout covers a page is a property of where the page is --
/// the rule Crystal's page_router uses for `layout.cr`.
///
/// The macro reads that off the directory tree at compile time: it knows
/// which file it was written in, walks up to `src/`, and wraps the page in
/// every `layout.rsx` it passes. Add one to a directory and every page under
/// it is wrapped, with nothing to register and no chain to keep in step.
///
/// Re-exported here so a page imports it from the same place as everything
/// else it uses, and so the layout rule lives beside the route table rather
/// than in a crate name.
pub use brocanth_pages::html;

/// The URL prefix a layout covers, for a layout under a parameter.
///
/// `html!` passes a layout the prefix it wraps, which is the marker dp-swap
/// reads to work out how much of a page two URLs share. A directory named
/// after a parameter has no prefix of its own until a request arrives, so
/// the macro hands this function the template and the request fills it:
/// `/{section}` under `/alpha/card-2` is `/alpha`.
///
/// Called only from the expansion, and only for a layout that sits under a
/// parameter. A marker reading `/[section]` would be the same string on
/// every section, and the client would conclude that two pages share a
/// layer they do not.
/// `allow(dead_code)`: `html!` emits a call to this only for a layout that
/// defines an `Oob` component, and no scaffolded layout does. It is here
/// for the first one that needs it, which is the same reason the request
/// readers on `Ctx` carry the attribute.
#[allow(dead_code)]
pub fn prefix(ctx: &Ctx, template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        // A parameter the router did not match cannot happen: this template
        // is built from the directory the route was read out of, so the
        // name is one the pattern declares.
        out.push_str(ctx.param(&rest[open + 1..open + close]).unwrap_or_default());
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

/// A partial, and the replacements for the chrome it left behind.
///
/// A layout the client keeps is a layout this answer does not contain, which
/// is the point of asking for a partial and the reason its chrome goes
/// stale: the sidebar still marks the section you came from. A layout that
/// has something to correct defines `Oob` beside its `Layout`, and `html!`
/// renders one per skipped layer and passes them here.
///
/// They lead, because the static version of this writes them that way. The
/// client reads the whole body before it swaps anything, so the order is a
/// convention rather than a requirement, and one convention is better than
/// two.
/// `allow(dead_code)`: `html!` emits a call to this only for a layout that
/// defines an `Oob` component, and no scaffolded layout does. It is here
/// for the first one that needs it, which is the same reason the request
/// readers on `Ctx` carry the attribute.
#[allow(dead_code)]
pub fn render_with(
    out_of_band: Vec<String>,
    content: impl rsx::Template,
) -> Result<Response, Error> {
    let mut body = out_of_band.concat();
    body.push_str(&content.render_to_string()?);
    Ok(Response::ok(body))
}

/// One skipped layout's replacement chrome, rendered on its own.
///
/// `html!` calls this once per layer it left out, and `render_with` puts
/// the results in front of the page. A function here rather than a call to
/// `rsx::Template` inside the macro: the other two renders already come
/// back through this file, and the macro naming one engine was the reason
/// a project could have only that engine.
/// `allow(dead_code)`: `html!` emits a call to this only for a layout that
/// defines an `Oob` component, and no scaffolded layout does. It is here
/// for the first one that needs it, which is the same reason the request
/// readers on `Ctx` carry the attribute.
#[allow(dead_code)]
pub fn render_oob(content: impl rsx::Template) -> Result<String, Error> {
    Ok(content.render_to_string()?)
}

/// A page with no layout at all: a fragment, a partial, an htmx swap.
///
/// What `html!` expands into, once it has wrapped the page, so this is also
/// the escape hatch -- name a layout by hand and pass it here.
///
/// It returns the handler's own `Result` rather than a value, so a template
/// that bails propagates with `?` instead of turning into a blank 200. For
/// another status or a header, compose with the forms above:
/// `Ok((html!(ctx, tpl)?, 404))`, or `Ok(html!(ctx, tpl)?.with_header("x-kind", "page"))`.
///
/// Deliberately a function and not `impl<T: rsx::Template> IntoResponse for T`.
/// `()`, `Option<T>` and `Vec<T>` are templates, so a blanket impl overlaps
/// the ones above and does not compile -- and an `into_response` that cannot
/// fail would have to swallow a render error. A blanket impl could not apply
/// a layout either, since it would not know which page it was rendering.
pub fn render(content: impl rsx::Template) -> Result<Response, Error> {
    // `render_to_string` builds into its own String, so a page that fails
    // half way through its layout leaves nothing behind. There is no partial
    // body to take back, which is the property the rest of this file is
    // built on.
    Ok(Response::ok(content.render_to_string()?))
}
"##;

fn layout_rsx(p: &Project) -> String {
    let mut head = String::new();
    if p.has("rsx-tailwind") {
        // `assets::url`, never the path by hand: the published name carries
        // a content hash, so writing it would be writing a name that changes
        // every time the stylesheet does.
        head.push_str(
            "            <link rel=\"stylesheet\" href={crate::assets::url(\"css/site.css\")}>\n",
        );
    }
    if p.has("rsx-htmx") {
        head.push_str(
            "            <script src={crate::assets::url(\"dpswap.min.js\")} defer></script>\n",
        );
    }
    let body_class = if p.has("rsx-tailwind") {
        " class=\"min-h-screen bg-white text-slate-900\""
    } else {
        ""
    };
    let main_open = if p.has("rsx-htmx") {
        // The marker dp-swap walks. It wraps exactly this layout's children
        // and nothing else, which is what lets the client find the deepest
        // layer both documents share.
        "<main dp-layout={layout} id=\"outlet\">"
    } else {
        "<main>"
    };
    LAYOUT_RSX
        .replace("@HEAD@\n", &head)
        .replace("@BODY_CLASS@", body_class)
        .replace("@MAIN@", main_open)
        .replace(
            "@PARAMS@",
            if p.has("rsx-htmx") {
                "layout: &str, "
            } else {
                ""
            },
        )
}

const LAYOUT_RSX: &str = r#"// The document every page in this directory, and every directory below it,
// is rendered inside. A page never names it: which layout wraps a page is a
// property of where the page is, which is the rule the Crystal page_router
// uses for layout.cr.
//
// A layout is an ordinary component with a parameter of type `Element`, and
// `children` is the only parameter `html!` fills -- it cannot know what a
// second one would mean. A value every page shares comes from a const, as
// the title does here; a value belonging to one request belongs on the page
// component, which the handler builds and which already has the context.

use crate::pages::TITLE;

pub fn Layout(@PARAMS@children: Element) -> Element {
    <!DOCTYPE html>
    <html lang="en">
        <head>
            <meta charset="utf-8">
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <title>{TITLE}</title>
@HEAD@
        </head>
        <body@BODY_CLASS@>
            <header><a href="/">{TITLE}</a></header>
            @MAIN@{children}</main>
        </body>
    </html>
}
"#;

/// `<Link>`: an anchor that swaps instead of reloading.
///
/// An ordinary `<a href>` first, enhanced second. With JavaScript off, with
/// a crawler, or on the first load of any page, it is a link and the browser
/// does what it always did. dp-swap turns it into a partial navigation only
/// where it can.
///
/// `to` takes what the route helpers return, so a link to a route that does
/// not exist does not compile -- `routes::users::id::path(7)`, never a
/// string. That is Crystal page_router's `UserPage.path(123)`.
const LINK_RSX: &str = r#"// An anchor that swaps instead of reloading the document.
//
// Written as a real <a href> and enhanced afterwards. With JavaScript off,
// to a crawler, and on the first load of any page it is a link and the
// browser does what it always did -- which is why `to` is required and why
// there is no version of this that renders a <span> with a click handler.
//
// `to` takes what the route helpers return:
//
//     <Link to={routes::users::id::path(7)}>User 7</Link>
//
// never a string. A route that does not exist is then a compile error
// rather than a 404 somebody finds later.

pub fn Link(to: &str, children: Element) -> Element {
    <a href={to} dp-boost>{children}</a>
}
"#;

/// The layout one directory down, to show that they stack.
fn users_layout_rsx(p: &Project) -> String {
    if p.has("rsx-htmx") {
        return USERS_LAYOUT_RSX
            .replace("@PARAMS@", "layout: &str, ")
            .replace("@MARK@", " dp-layout={layout}");
    }
    USERS_LAYOUT_RSX
        .replace("@PARAMS@", "")
        .replace("@MARK@", "")
}

const USERS_LAYOUT_RSX: &str = r#"// Wraps every page under src/pages/users/, inside the root layout rather
// than instead of it. Nothing declares the stacking: `html!` reads it off
// the directory tree.
//
// `layout` is the URL prefix this layout covers, filled in by `html!`, and
// `dp-layout` is the marker dp-swap walks to find the deepest layer two
// pages share. It goes on the element wrapping exactly the children and
// nothing else: anything inside it is replaced along with them, so a
// sidebar in here would lose its scroll position on every navigation.

pub fn Layout(@PARAMS@children: Element) -> Element {
    <section class="users">
        <nav><a href="/">home</a></nav>
        <div@MARK@>{children}</div>
    </section>
}
"#;

fn index_rsx(p: &Project) -> String {
    if p.has("rsx-htmx") {
        return INDEX_RSX
            .replace("@USE@", "use crate::pages::views::link::Link;\n\n")
            .replace(
                "<a href=\"/about\">/about</a>",
                "<Link to=\"/about\">/about</Link>",
            )
            .replace(
                "<a href=\"/users/7\">/users/7</a>",
                "<Link to={&crate::pages::routes::users::id::path(7)}>User 7</Link>",
            );
    }
    INDEX_RSX.replace("@USE@", "")
}

const INDEX_RSX: &str = r#"@USE@// The page, and only the page. The document around it is layout.rsx, and
// this file does not know it exists.

pub fn Index(title: &str) -> Element {
    <h1>{title}</h1>
    <p>
        This file is <code>src/pages/index.rsx</code>, beside the handler
        in <code>index.rs</code>, and it is compiled into the binary. Edit it
        and cargo rebuilds; break it and the build stops on the line you broke.
    </p>
    <p>
        An interpolated value is escaped on the way out, always. Writing
        markup verbatim needs a different and greppable hole, so an injection
        is something you can search for rather than something you have to
        remember.
    </p>
    <ul>
        <li><a href="/about">/about</a></li>
        <li><a href="/users/7">/users/7</a></li>
    </ul>
}
"#;

const ABOUT_RSX: &str = r#"// The parameters are this page's contract with its handler. Rename one and
// src/pages/about.rs stops compiling, which is the point.

pub fn About(title: &str, verb: &str, path: &str) -> Element {
    <h1>About</h1>
    <p>
        You asked for <code>{verb} {path}</code>. This page answers GET
        and nothing else, so a DELETE to it is a 405 carrying
        an <code>Allow</code> header rather than a 404.
    </p>
    <p>The shell around this is <code>src/pages/layout.rsx</code>, which {title} shares with every page.</p>
}
"#;

const USER_RSX: &str = r#"// Two layouts wrap this one: the root, and users/layout.rsx beside it.
// Neither is named here or anywhere else -- `html!` reads the directory
// tree. Delete users/layout.rsx and this page is wrapped in one.

pub fn User(id: u64) -> Element {
    <h1>User {id}</h1>
    <p>
        The <code>id</code> came out of the path. A component takes typed
        parameters, so this one is a <code>u64</code> the whole way
        from <code>src/pages/users/[id].rs</code>, never a string that
        might not be a number.
    </p>
}
"#;

// -- tailwind ----------------------------------------------------------------

/// `src/assets.rs`: two directories, embedded.
const ASSETS_RS: &str = r#"//! Every static file this binary serves.
//!
//! Two directories, two rules. `assets/` is **fingerprinted** -- `site.css`
//! is published at `/css/site.<hash>.css`, so it can be cached forever and a
//! new build invalidates it by having a different name. `public/` is served
//! **verbatim**, because `robots.txt`, `favicon.ico` and anything linked to
//! from outside must keep the name they were given.
//!
//! Write a reference with `assets::url`, never by hand:
//!
//! ```ignore
//! <link rel="stylesheet" href={assets::url("css/site.css")}>
//! ```
//!
//! A name nothing publishes panics there rather than 404ing in a browser.
//!
//! Embedded, like the templates and the route table, so `serve` works from
//! any directory and nothing can go missing after the build passed. A file
//! added where there was none needs `touch src/assets.rs`, because nothing
//! referenced it yet.

brocanth_assets::assets!("assets", "public");
"#;

/// dp-swap, the client half of partial navigation.
///
/// Embedded rather than fetched: `brocanth create` has to finish on a
/// machine with no network, and a scaffold that sometimes leaves a file out
/// is worse than a binary a few kilobytes larger. See `vendor/README.md`.
const DPSWAP_JS: &str = include_str!("../../vendor/dpswap.min.js");

/// The input of the CSS build. Never published: the leading underscore is
/// the mark `assets!` reads for a file that is bundled into another.
const TAILWIND_CSS: &str = r#"/* The input of the CSS build. Never published.
 *
 * The leading underscore marks an input: a file that is bundled into another
 * and that `assets!` must not publish on its own. The build reads this file
 * and writes assets/css/site.css, which is the one that ships:
 *
 *     brocanth css          # once, minified
 *     brocanth css -W       # again on each change
 */

@import "tailwindcss";

/* Where the class names are.
 *
 * Tailwind resolves each path against this stylesheet and keeps its automatic
 * detection on as well. These are explicit for two reasons: `.rsx` is not an
 * extension Tailwind knows, and an explicit glob states the contract.
 */

/* The templates. A .rsx file is Rust with an HTML-shaped body. */
@source "../../src/pages/**/*.rsx";

/* Plain Rust holds class names too -- a helper that builds a class string,
 * or a handler choosing one. */
@source "../../src/**/*.rs";
"#;

/// Written so the first `cargo build` works before anyone has run node.
///
/// Without it `assets::url("css/site.css")` names a file nothing publishes,
/// and a brand-new project would fail on its first request with a message
/// about an asset rather than about node. depoth's build script writes
/// placeholders for the same reason.
const SITE_CSS_PLACEHOLDER: &str = "/* Written by `brocanth create` so the \
                                    first build works.\n   `brocanth css` \
                                    replaces it with the real one. */\n";

/// `src/commands/css.rs`: the CSS build, as a subcommand of this project.
///
/// A shell script was the obvious thing and the wrong one. The project
/// already has a command line whose subcommands register themselves at link
/// time, so the build belongs there: it gets `--help` for free, it is found
/// by `brocanth --help` beside `serve`, and it is Rust that a reader can
/// change without leaving the language the rest of the project is in.
const CSS_RS: &str = r##"//! `css` -- build assets/css/site.css from assets/css/_tailwind.css.
//!
//!     brocanth css          once, minified
//!     brocanth css -W       again on each change
//!
//! Ordinary project code, like `serve`: rename it, give it more flags, or
//! delete it. It is here rather than in a shell script because this project
//! already has a command line, and a second way to run things is a second
//! thing to keep in step.
//!
//! The Tailwind CLI comes out of `node_modules` and there is no fallback to
//! `npx`. A fallback downloads some other version and hides the one fact
//! worth knowing, which is that nobody ran `npm install`.

use std::path::Path;
use std::process::Command as Child;

use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// Build the stylesheet from the Tailwind input.
#[cmd]
#[derive(clap::Args)]
pub struct Css {
    /// Rebuild on each change instead of once.
    #[arg(short = 'W', long)]
    pub watch: bool,
}

const INPUT: &str = "assets/css/_tailwind.css";
const OUTPUT: &str = "assets/css/site.css";
const CLI: &str = "node_modules/.bin/tailwindcss";

impl Command for Css {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        // The project root and not the working directory: `brocanth css`
        // runs from wherever you happen to be, and every path below is
        // written relative to the project.
        let root = ctx.root();
        let cli = root.join(CLI);
        if !cli.is_file() {
            anyhow::bail!(
                "the Tailwind CLI is absent at {}\n       run `npm install` in {} first",
                cli.display(),
                root.display()
            );
        }

        let mut child = Child::new(&cli);
        child
            .current_dir(root)
            .args(["--input", INPUT, "--output", OUTPUT]);
        if self.watch {
            child.arg("--watch");
        } else {
            child.arg("--minify");
        }

        let status = child.status()?;
        if !status.success() {
            // No message beyond this: the CLI has already written its own
            // on the stderr this process inherited.
            anyhow::bail!("the Tailwind CLI failed");
        }

        // `--watch` only ever returns when it was interrupted, and the
        // output it leaves behind is whatever the last change produced.
        // Checking it, or touching the module below, would both be about a
        // build that is over.
        if self.watch {
            return Ok(());
        }

        let written = root.join(OUTPUT);
        let bytes = std::fs::metadata(&written)?.len();
        if bytes == 0 {
            anyhow::bail!("the build wrote no CSS to {}", written.display());
        }

        touch(&root.join("src/assets.rs"))?;
        println!("built {OUTPUT} ({bytes} bytes)");
        Ok(())
    }
}

/// Make cargo notice a stylesheet it has already compiled in.
///
/// The CSS is embedded at compile time, so a new one is a new binary, and
/// nothing rebuilds until the module that embeds it looks newer than the
/// artefact. Rewriting its own bytes is the portable way to say so: there is
/// no `utimes` in `std`, and a dependency for one call is not worth it.
fn touch(path: &Path) -> anyhow::Result<()> {
    let same = std::fs::read(path)?;
    std::fs::write(path, same)?;
    Ok(())
}
"##;

fn package_json(p: &Project) -> String {
    format!(
        r#"{{
  "name": {name:?},
  "version": "0.0.0",
  "private": true,
  "description": "One dev dependency: the Tailwind CLI. `brocanth css` runs it.",
  "devDependencies": {{
    "@tailwindcss/cli": "^4.1.14"
  }}
}}
"#,
        name = p.name
    )
}

/// The two `[tasks.*]` tables the CSS build adds to `mise.toml`.
const MISE_CSS_TASKS: &str = r#"[tasks.css]
description = "build assets/css/site.css from _tailwind.css"
run = "cargo run -- css"

[tasks."css:watch"]
description = "the same, again on each change"
run = "cargo run -- css --watch"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_template_names_its_own_shell() {
        // The Crystal page_router's rule: which layout wraps a page is a
        // property of where the page is. `html!` reads that off the
        // directory tree, so a chain written down anywhere would be a
        // chain that can disagree with the tree.
        for template in [INDEX_RSX, ABOUT_RSX, USER_RSX] {
            assert!(
                !template.contains("<Layout"),
                "a page names its own shell:\n{template}"
            );
        }
    }

    #[test]
    fn the_renderer_block_declares_the_views_module() {
        // What a second engine copies. The `pub mod` is the whole seam:
        // `load_dir!` emits a `pub mod index` for `index.rsx` and `pages!`
        // emits one for `index.rs`, so unnested they collide.
        assert!(PAGES_RSX_RS.contains("pub mod views {"), "{PAGES_RSX_RS}");
        assert!(
            PAGES_RSX_RS.contains("rsx::load_dir!(\"src/pages\");"),
            "{PAGES_RSX_RS}"
        );
    }
}
